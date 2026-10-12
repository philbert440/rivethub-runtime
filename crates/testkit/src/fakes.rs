use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::ToolContext;
use kernel::harness::HarnessAdapter;
use kernel::harness::HarnessAdapterError;
use kernel::harness::SessionEvent;
use kernel::harness::SessionOption;
use kernel::harness::SessionSpec;
use kernel::harness::TurnInput;
use kernel::memory::BrowseFilter;
use kernel::memory::MemoryStore;
use kernel::memory::MemoryStoreError;
use kernel::memory::SearchQuery;
use kernel::memory::TagAction;
use kernel::memory::TagOutcome;
use kernel::model::Embedding;
use kernel::model::ModelChunk;
use kernel::model::ModelProvider;
use kernel::model::ModelProviderError;
use kernel::model::ModelRequest;
use kernel::queue::ClaimedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use kernel::terminal::PtyInfo;
use kernel::terminal::TermSize;
use kernel::terminal::TerminalId;
use kernel::terminal::TerminalMux;
use kernel::terminal::TerminalMuxError;
use kernel::tool::Tool;
use kernel::tool::ToolError;
use kernel::transcript::TranscriptReader;
use kernel::transcript::TranscriptReaderError;
use kernel::transcript::Turn;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;
use wire::gateway::memory::MemoryStatsResponse;
use wire::harness::ApprovalDecision;
use wire::memory::MemoryEntry;
use wire::memory::MemorySearchResult;
use wire::session_id::SessionId;
use wire::stream::LlmResponse;

#[derive(Debug, Default)]
pub struct FakeMemoryStore {
    entries: Mutex<Vec<MemoryEntry>>,
}

#[async_trait]
impl MemoryStore for FakeMemoryStore {
    async fn append(&self, batch: Vec<MemoryEntry>) -> Result<u64, MemoryStoreError> {
        let count = u64::try_from(batch.len()).unwrap_or(u64::MAX);
        self.entries.lock().await.extend(batch);
        Ok(count)
    }

    async fn search(
        &self,
        query: SearchQuery,
    ) -> Result<Vec<MemorySearchResult>, MemoryStoreError> {
        let entries = self.entries.lock().await;
        Ok(entries
            .iter()
            .filter(|e| e.content.contains(query.text.trim()))
            .map(|e| MemorySearchResult {
                id: e.id.clone().unwrap_or_default(),
                content: e.content.clone(),
                role: e.role,
                agent: e.agent.clone(),
                relevance_score: 1.0,
                created_at: e.created_at.unwrap_or_default(),
            })
            .take(usize::try_from(query.limit).unwrap_or(0))
            .collect())
    }

    async fn browse(&self, filter: BrowseFilter) -> Result<Vec<MemoryEntry>, MemoryStoreError> {
        let entries = self.entries.lock().await;
        Ok(entries
            .iter()
            .filter(|e| {
                filter
                    .session_key
                    .as_deref()
                    .is_none_or(|k| e.session_id == k)
            })
            .take(usize::try_from(filter.limit).unwrap_or(0))
            .cloned()
            .collect())
    }

    async fn stats(&self) -> Result<MemoryStatsResponse, MemoryStoreError> {
        let count = u64::try_from(self.entries.lock().await.len()).unwrap_or(u64::MAX);
        Ok(MemoryStatsResponse {
            conversations: 1,
            messages: count,
            tool_calls: 0,
            summaries: 0,
            embed_queue_depth: 0,
            embedded_messages: 0,
            failed_embeddings: 0,
            top_tools: Vec::new(),
            recent_sessions: Vec::new(),
        })
    }

    async fn get_full(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryStoreError> {
        Ok(self
            .entries
            .lock()
            .await
            .iter()
            .find(|e| e.id.as_deref() == Some(id))
            .cloned())
    }

    async fn tags(&self, _action: TagAction) -> Result<TagOutcome, MemoryStoreError> {
        Ok(TagOutcome {
            moved: 0,
            dropped: 0,
        })
    }

    async fn context(
        &self,
        session: &str,
        _now: DateTime<Utc>,
    ) -> Result<String, MemoryStoreError> {
        let entries = self.entries.lock().await;
        Ok(entries
            .iter()
            .filter(|e| e.session_id == session)
            .map(|e| e.content.clone())
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

#[derive(Debug, Default)]
pub struct FakeJobQueue {
    jobs: Mutex<BTreeMap<JobId, Job>>,
    next_id: AtomicI64,
}

#[async_trait]
impl JobQueue for FakeJobQueue {
    async fn enqueue(&self, job: Job) -> Result<(), JobQueueError> {
        let id = JobId::new(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.jobs.lock().await.insert(id, job);
        Ok(())
    }

    async fn claim(&self, _role: WorkerRole) -> Result<Option<ClaimedJob>, JobQueueError> {
        let claimed = {
            let mut jobs = self.jobs.lock().await;
            jobs.iter()
                .next()
                .map(|(id, job)| (*id, job.clone()))
                .map(|(id, job)| {
                    jobs.remove(&id);
                    ClaimedJob {
                        id,
                        name: job.name,
                        payload: job.payload,
                        attempt: 1,
                    }
                })
        };
        Ok(claimed)
    }

    async fn complete(&self, id: JobId) -> Result<(), JobQueueError> {
        self.jobs.lock().await.remove(&id);
        Ok(())
    }

    async fn fail(&self, _id: JobId, _error: &str) -> Result<(), JobQueueError> {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum Script {
    Chat(LlmResponse),
    Stream(Vec<ModelChunk>),
    Embed(Vec<Embedding>),
    Models(Vec<String>),
}

#[derive(Debug, Default)]
pub struct FakeProvider {
    script: Mutex<Vec<Script>>,
}

impl FakeProvider {
    pub fn new(script: Vec<Script>) -> Self {
        Self {
            script: Mutex::new(script),
        }
    }

    async fn scripted<T>(&self, keep: fn(&Script) -> Option<T>) -> Option<T> {
        let mut script = self.script.lock().await;
        let index = script.iter().position(|s| keep(s).is_some())?;
        let entry = script.get(index).cloned()?;
        let chosen = keep(&entry);
        script.remove(index);
        drop(script);
        chosen
    }
}

fn default_response() -> LlmResponse {
    LlmResponse {
        r#type: wire::stream::LlmResponseKind::Text,
        content: Some("ok".to_string()),
        tool_calls: None,
        usage: None,
    }
}

#[async_trait]
impl ModelProvider for FakeProvider {
    async fn chat(&self, _request: ModelRequest) -> Result<LlmResponse, ModelProviderError> {
        Ok(self
            .scripted(|s| match s {
                Script::Chat(response) => Some(response.clone()),
                Script::Stream(_) | Script::Embed(_) | Script::Models(_) => None,
            })
            .await
            .unwrap_or_else(default_response))
    }

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> Result<Receiver<ModelChunk>, ModelProviderError> {
        let chunks = self
            .scripted(|s| match s {
                Script::Stream(chunks) => Some(chunks.clone()),
                Script::Chat(_) | Script::Embed(_) | Script::Models(_) => None,
            })
            .await;
        let chunks = chunks.unwrap_or_else(|| {
            vec![
                ModelChunk::Text {
                    delta: request
                        .messages
                        .last()
                        .map(|m| match &m.content {
                            wire::message::MessageContent::Text(t) => t.clone(),
                            wire::message::MessageContent::Parts(_) => String::new(),
                        })
                        .unwrap_or_default(),
                },
                ModelChunk::Done,
            ]
        });
        let (tx, rx) = tokio::sync::mpsc::channel(chunks.len().max(1));
        for chunk in chunks {
            let _ = tx.try_send(chunk);
        }
        Ok(rx)
    }

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        Ok(self
            .scripted(|s| match s {
                Script::Embed(vectors) => Some(vectors.clone()),
                Script::Chat(_) | Script::Stream(_) | Script::Models(_) => None,
            })
            .await
            .unwrap_or_else(|| {
                texts
                    .into_iter()
                    .map(|t| Embedding {
                        vector: vec![f32::from(u16::try_from(t.len()).unwrap_or(u16::MAX))],
                    })
                    .collect()
            }))
    }

    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError> {
        Ok(self
            .scripted(|s| match s {
                Script::Models(models) => Some(models.clone()),
                Script::Chat(_) | Script::Stream(_) | Script::Embed(_) => None,
            })
            .await
            .unwrap_or_else(|| vec!["fake-model".to_string()]))
    }
}

#[derive(Debug)]
struct SessionStream {
    tx: tokio::sync::mpsc::Sender<SessionEvent>,
    rx: Option<Receiver<SessionEvent>>,
}

#[derive(Debug, Default)]
pub struct FakeHarness {
    queued: Mutex<Vec<SessionEvent>>,
    sessions: Mutex<BTreeMap<SessionId, SessionStream>>,
}

impl FakeHarness {
    pub fn new(events: Vec<SessionEvent>) -> Self {
        Self {
            queued: Mutex::new(events),
            sessions: Mutex::new(BTreeMap::new()),
        }
    }
}

#[async_trait]
impl HarnessAdapter for FakeHarness {
    async fn new_session(&self, spec: SessionSpec) -> Result<SessionId, HarnessAdapterError> {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        self.sessions
            .lock()
            .await
            .insert(spec.session.clone(), SessionStream { tx, rx: Some(rx) });
        Ok(spec.session)
    }

    async fn resume(&self, session: &SessionId) -> Result<(), HarnessAdapterError> {
        let mut sessions = self.sessions.lock().await;
        if !sessions.contains_key(session) {
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            sessions.insert(session.clone(), SessionStream { tx, rx: Some(rx) });
        }
        drop(sessions);
        Ok(())
    }

    async fn prompt(
        &self,
        session: &SessionId,
        _input: TurnInput,
    ) -> Result<(), HarnessAdapterError> {
        let events: Vec<SessionEvent> = self.queued.lock().await.drain(..).collect();
        let tx = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session)
                .ok_or_else(|| HarnessAdapterError::UnknownSession(session.clone()))?
                .tx
                .clone()
        };
        for event in events {
            let _ = tx.send(event).await;
        }
        Ok(())
    }

    async fn cancel(&self, _session: &SessionId) -> Result<(), HarnessAdapterError> {
        Ok(())
    }

    async fn answer_permission(
        &self,
        _session: &SessionId,
        _request_id: &str,
        _decision: ApprovalDecision,
    ) -> Result<(), HarnessAdapterError> {
        Ok(())
    }

    async fn set_option(
        &self,
        _session: &SessionId,
        _option: SessionOption,
    ) -> Result<(), HarnessAdapterError> {
        Ok(())
    }

    async fn events(
        &self,
        session: &SessionId,
    ) -> Result<Receiver<SessionEvent>, HarnessAdapterError> {
        let mut sessions = self.sessions.lock().await;
        let stream = sessions
            .get_mut(session)
            .ok_or_else(|| HarnessAdapterError::UnknownSession(session.clone()))?;
        let taken = stream.rx.take();
        drop(sessions);
        taken.ok_or_else(|| HarnessAdapterError::EventStreamTaken(session.clone()))
    }
}

#[derive(Debug, Default)]
pub struct FakeMux {
    ptys: Mutex<Vec<(TerminalId, SessionId, bool)>>,
    next_id: AtomicI64,
}

#[async_trait]
impl TerminalMux for FakeMux {
    async fn spawn(
        &self,
        session: &SessionId,
        _argv: Vec<String>,
        _cwd: &std::path::Path,
        _size: TermSize,
    ) -> Result<TerminalId, TerminalMuxError> {
        let id = TerminalId::new(format!(
            "pty-{}",
            self.next_id.fetch_add(1, Ordering::Relaxed)
        ));
        self.ptys
            .lock()
            .await
            .push((id.clone(), session.clone(), true));
        Ok(id)
    }

    async fn attach(&self, id: &TerminalId) -> Result<Receiver<Vec<u8>>, TerminalMuxError> {
        self.ptys
            .lock()
            .await
            .iter()
            .find(|(pty, _, _)| pty == id)
            .ok_or_else(|| TerminalMuxError::UnknownTerminal(id.clone()))?;
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        Ok(rx)
    }

    async fn input(&self, id: &TerminalId, _bytes: Vec<u8>) -> Result<(), TerminalMuxError> {
        self.ptys
            .lock()
            .await
            .iter()
            .find(|(pty, _, _)| pty == id)
            .ok_or_else(|| TerminalMuxError::UnknownTerminal(id.clone()))?;
        Ok(())
    }

    async fn resize(&self, id: &TerminalId, _size: TermSize) -> Result<(), TerminalMuxError> {
        self.ptys
            .lock()
            .await
            .iter()
            .find(|(pty, _, _)| pty == id)
            .ok_or_else(|| TerminalMuxError::UnknownTerminal(id.clone()))?;
        Ok(())
    }

    async fn kill(&self, id: &TerminalId) -> Result<(), TerminalMuxError> {
        let killed = {
            let mut ptys = self.ptys.lock().await;
            match ptys.iter_mut().find(|(pty, _, _)| pty == id) {
                Some(entry) => {
                    entry.2 = false;
                    true
                }
                None => false,
            }
        };
        if killed {
            Ok(())
        } else {
            Err(TerminalMuxError::UnknownTerminal(id.clone()))
        }
    }

    async fn list(&self) -> Result<Vec<PtyInfo>, TerminalMuxError> {
        Ok(self
            .ptys
            .lock()
            .await
            .iter()
            .map(|(id, session, running)| PtyInfo {
                id: id.clone(),
                session: session.clone(),
                running: *running,
            })
            .collect())
    }
}

#[derive(Debug)]
pub struct FakeTool {
    name: String,
}

impl FakeTool {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Tool for FakeTool {
    fn definition(&self) -> rmcp::model::Tool {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"]
        });
        rmcp::model::Tool::new(
            self.name.clone(),
            "echo the text argument back",
            schema.as_object().cloned().unwrap_or_default(),
        )
    }

    async fn call(
        &self,
        arguments: Option<rmcp::model::JsonObject>,
        _ctx: ToolContext,
    ) -> Result<rmcp::model::CallToolResult, ToolError> {
        let text = arguments
            .as_ref()
            .and_then(|args| args.get("text"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("text is required".to_string()))?;
        Ok(rmcp::model::CallToolResult::success(vec![
            rmcp::model::ContentBlock::text(text),
        ]))
    }
}

#[derive(Debug)]
pub struct FakeReader {
    turns: BTreeMap<String, Vec<Turn>>,
}

impl FakeReader {
    pub fn new(turns: BTreeMap<String, Vec<Turn>>) -> Self {
        Self { turns }
    }
}

#[async_trait]
impl TranscriptReader for FakeReader {
    async fn sessions(
        &self,
        _store: &std::path::Path,
    ) -> Result<Vec<SessionId>, TranscriptReaderError> {
        Ok(self
            .turns
            .keys()
            .filter_map(|k| SessionId::from_str(k).ok())
            .collect())
    }

    async fn read(&self, session: &SessionId) -> Result<Vec<Turn>, TranscriptReaderError> {
        Ok(self
            .turns
            .get(&session.to_string())
            .cloned()
            .unwrap_or_default())
    }

    async fn tail(
        &self,
        session: &SessionId,
        since: DateTime<Utc>,
    ) -> Result<Vec<Turn>, TranscriptReaderError> {
        Ok(self
            .read(session)
            .await?
            .into_iter()
            .filter(|turn| turn.at > since)
            .collect())
    }
}
