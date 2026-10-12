use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::ToolContext;
use kernel::harness::Command;
use kernel::harness::HarnessAdapter;
use kernel::harness::HarnessAdapterError;
use kernel::harness::PlanEntry;
use kernel::harness::PlanPriority;
use kernel::harness::SessionEvent;
use kernel::harness::SessionOption;
use kernel::harness::SessionSpec;
use kernel::harness::ToolCallInfo;
use kernel::harness::ToolCallStatus;
use kernel::harness::ToolCallUpdate;
use kernel::harness::TurnInput;
use kernel::memory::BrowseFilter;
use kernel::memory::MemoryStore;
use kernel::memory::MemoryStoreError;
use kernel::memory::SearchQuery;
use kernel::memory::TagAction;
use kernel::model::Embedding;
use kernel::model::ModelChunk;
use kernel::model::ModelProvider;
use kernel::model::ModelProviderError;
use kernel::model::ModelRequest;
use kernel::queue::ClaimedJob;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use kernel::terminal::TermSize;
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
use wire::harness::ApprovalDecision as Decision;
use wire::memory::MemoryEntry;
use wire::memory::MemorySearchResult;
use wire::session_id::SessionId;

#[derive(Debug, Default)]
pub struct FakeMemoryStore {
    entries: Mutex<Vec<MemoryEntry>>,
}

impl FakeMemoryStore {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
        }
    }
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

    async fn tags(
        &self,
        _action: TagAction,
    ) -> Result<kernel::memory::TagOutcome, MemoryStoreError> {
        Ok(kernel::memory::TagOutcome {
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
    jobs: Mutex<BTreeMap<i64, Job>>,
    next_id: Mutex<i64>,
}

impl FakeJobQueue {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(BTreeMap::new()),
            next_id: Mutex::new(1),
        }
    }
}

#[async_trait]
impl JobQueue for FakeJobQueue {
    async fn enqueue(&self, job: Job) -> Result<(), JobQueueError> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = *next;
            *next += 1;
            id
        };
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

    async fn complete(&self, id: i64) -> Result<(), JobQueueError> {
        self.jobs.lock().await.remove(&id);
        Ok(())
    }

    async fn fail(&self, _id: i64, _error: &str) -> Result<(), JobQueueError> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct FakeModelProvider;

#[async_trait]
impl ModelProvider for FakeModelProvider {
    async fn chat(
        &self,
        _request: ModelRequest,
    ) -> Result<wire::stream::LlmResponse, ModelProviderError> {
        Ok(wire::stream::LlmResponse {
            r#type: wire::stream::LlmResponseKind::Text,
            content: Some("ok".to_string()),
            tool_calls: None,
            usage: None,
        })
    }

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> Result<Receiver<ModelChunk>, ModelProviderError> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let text = request
            .messages
            .last()
            .map(|m| match &m.content {
                wire::message::MessageContent::Text(t) => t.clone(),
                wire::message::MessageContent::Parts(_) => String::new(),
            })
            .unwrap_or_default();
        tokio::spawn(async move {
            let _ = tx.send(ModelChunk::Text { delta: text }).await;
            let _ = tx.send(ModelChunk::Done).await;
        });
        Ok(rx)
    }

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        Ok(texts
            .into_iter()
            .map(|t| Embedding {
                vector: vec![f32::from(u16::try_from(t.len()).unwrap_or(u16::MAX))],
            })
            .collect())
    }

    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError> {
        Ok(vec!["fake-model".to_string()])
    }
}

struct SessionStream {
    tx: tokio::sync::mpsc::Sender<SessionEvent>,
    rx: Option<Receiver<SessionEvent>>,
}

#[derive(Debug, Default)]
pub struct FakeHarnessAdapter {
    queued: Mutex<Vec<SessionEvent>>,
    sessions: Mutex<BTreeMap<SessionId, SessionStream>>,
}

impl std::fmt::Debug for SessionStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStream")
            .field("rx_taken", &self.rx.is_none())
            .finish_non_exhaustive()
    }
}

impl FakeHarnessAdapter {
    pub fn new(events: Vec<SessionEvent>) -> Self {
        Self {
            queued: Mutex::new(events),
            sessions: Mutex::new(BTreeMap::new()),
        }
    }
}

#[async_trait]
impl HarnessAdapter for FakeHarnessAdapter {
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
                .ok_or_else(|| HarnessAdapterError::UnknownSession(session.to_string()))?
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
        _decision: Decision,
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
            .ok_or_else(|| HarnessAdapterError::UnknownSession(session.to_string()))?;
        let taken = stream.rx.take();
        drop(sessions);
        taken.ok_or_else(|| HarnessAdapterError::EventStreamTaken(session.to_string()))
    }
}

#[derive(Debug, Default)]
pub struct FakeTerminalMux {
    ptys: Mutex<Vec<(String, SessionId, bool)>>,
    next_id: Mutex<i64>,
}

impl FakeTerminalMux {
    pub fn new() -> Self {
        Self {
            ptys: Mutex::new(Vec::new()),
            next_id: Mutex::new(1),
        }
    }
}

#[async_trait]
impl TerminalMux for FakeTerminalMux {
    async fn spawn(
        &self,
        session: &SessionId,
        _argv: Vec<String>,
        _cwd: &std::path::Path,
        _size: TermSize,
    ) -> Result<String, TerminalMuxError> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = format!("pty-{}", *next);
            *next += 1;
            id
        };
        self.ptys
            .lock()
            .await
            .push((id.clone(), session.clone(), true));
        Ok(id)
    }

    async fn attach(&self, id: &str) -> Result<Receiver<Vec<u8>>, TerminalMuxError> {
        self.ptys
            .lock()
            .await
            .iter()
            .find(|(pty, _, _)| pty == id)
            .ok_or_else(|| TerminalMuxError::UnknownTerminal(id.to_string()))?;
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        Ok(rx)
    }

    async fn input(&self, id: &str, _bytes: Vec<u8>) -> Result<(), TerminalMuxError> {
        self.ptys
            .lock()
            .await
            .iter()
            .find(|(pty, _, _)| pty == id)
            .ok_or_else(|| TerminalMuxError::UnknownTerminal(id.to_string()))?;
        Ok(())
    }

    async fn resize(&self, id: &str, _size: TermSize) -> Result<(), TerminalMuxError> {
        self.ptys
            .lock()
            .await
            .iter()
            .find(|(pty, _, _)| pty == id)
            .ok_or_else(|| TerminalMuxError::UnknownTerminal(id.to_string()))?;
        Ok(())
    }

    async fn kill(&self, id: &str) -> Result<(), TerminalMuxError> {
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
            Err(TerminalMuxError::UnknownTerminal(id.to_string()))
        }
    }

    async fn list(&self) -> Result<Vec<kernel::terminal::PtyInfo>, TerminalMuxError> {
        Ok(self
            .ptys
            .lock()
            .await
            .iter()
            .map(|(id, session, running)| kernel::terminal::PtyInfo {
                id: id.clone(),
                session: session.clone(),
                running: *running,
            })
            .collect())
    }
}

#[derive(Debug)]
pub struct EchoTool {
    name: String,
}

impl EchoTool {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl Tool for EchoTool {
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
pub struct FakeTranscriptReader {
    turns: BTreeMap<String, Vec<Turn>>,
}

impl FakeTranscriptReader {
    pub fn new(turns: BTreeMap<String, Vec<Turn>>) -> Self {
        Self { turns }
    }
}

#[async_trait]
impl TranscriptReader for FakeTranscriptReader {
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

fn memory_entry(session: &str, content: &str, at: DateTime<Utc>) -> MemoryEntry {
    MemoryEntry {
        id: Some(format!("m-{}", content.len())),
        session_id: session.to_string(),
        agent: "agent-a".to_string(),
        channel: "test".to_string(),
        role: wire::message::Role::User,
        content: content.to_string(),
        tool_name: None,
        tool_args: None,
        tool_result: None,
        metadata: None,
        created_at: Some(at),
    }
}

#[tokio::test]
async fn memory_store_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let store = FakeMemoryStore::new();
    let bounded: std::sync::Arc<dyn MemoryStore> = std::sync::Arc::new(store);
    let now = Utc::now();
    let inserted = bounded
        .append(vec![memory_entry("claude-code:abc", "hello world", now)])
        .await?;
    assert_eq!(inserted, 1);
    let hits = bounded
        .search(SearchQuery {
            text: "hello".to_string(),
            agent: None,
            limit: 10,
        })
        .await?;
    assert_eq!(hits.len(), 1);
    let stats = bounded.stats().await?;
    assert_eq!(stats.messages, 1);
    let browsed = bounded
        .browse(BrowseFilter {
            session_key: Some("claude-code:abc".to_string()),
            agent: None,
            limit: 10,
        })
        .await?;
    assert_eq!(browsed.len(), 1);
    let full = bounded.get_full("m-11").await?;
    assert!(full.is_some());
    let context = bounded.context("claude-code:abc", now).await?;
    assert_eq!(context, "hello world");
    Ok(())
}

#[tokio::test]
async fn job_queue_claims_in_order() -> Result<(), Box<dyn std::error::Error>> {
    let queue: std::sync::Arc<dyn JobQueue> = std::sync::Arc::new(FakeJobQueue::new());
    queue
        .enqueue(Job {
            name: JobName::EmbedTarget,
            payload: wire::JsonObject::default(),
            key: Some("task:1".to_string()),
            key_mode: JobKeyMode::Replace,
            priority: 0,
            run_at: None,
        })
        .await?;
    let claimed = queue
        .claim(WorkerRole::Embedder)
        .await?
        .ok_or("expected one queued job")?;
    assert_eq!(claimed.name, JobName::EmbedTarget);
    queue.complete(claimed.id).await?;
    assert!(queue.claim(WorkerRole::Embedder).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn model_provider_streams_text() -> Result<(), Box<dyn std::error::Error>> {
    let provider: std::sync::Arc<dyn ModelProvider> = std::sync::Arc::new(FakeModelProvider);
    let request = ModelRequest {
        model: "fake-model".to_string(),
        messages: vec![wire::message::Message {
            role: wire::message::Role::User,
            content: wire::message::MessageContent::Text("hi".to_string()),
            tool_calls: None,
            tool_call_id: None,
        }],
        tools: Vec::new(),
        thinking: Some(wire::stream::ThinkingLevel::Medium),
        max_tokens: None,
        temperature: None,
    };
    let mut rx = provider.stream(request).await?;
    let first = rx.recv().await.ok_or("expected a chunk")?;
    assert_eq!(
        first,
        ModelChunk::Text {
            delta: "hi".to_string()
        }
    );
    let embeddings = provider.embed(vec!["abc".to_string()]).await?;
    assert_eq!(
        embeddings.first().map(|e| e.vector.clone()),
        Some(vec![3.0_f32])
    );
    let models = provider.list_models().await?;
    assert_eq!(models, vec!["fake-model".to_string()]);
    Ok(())
}

#[tokio::test]
async fn harness_adapter_delivers_session_events() -> Result<(), Box<dyn std::error::Error>> {
    let session = SessionId::from_str("grok-build:native-1")?;
    let events = vec![
        SessionEvent::ToolCall(ToolCallInfo {
            tool_call_id: "t1".to_string(),
            name: "shell".to_string(),
            input: wire::JsonObject::default(),
        }),
        SessionEvent::ToolCallUpdate(ToolCallUpdate {
            tool_call_id: "t1".to_string(),
            status: ToolCallStatus::Completed,
            output: Some("done".to_string()),
        }),
        SessionEvent::Plan(vec![PlanEntry {
            content: "step".to_string(),
            priority: PlanPriority::High,
        }]),
        SessionEvent::AvailableCommands(vec![Command {
            name: "compact".to_string(),
            description: "compact context".to_string(),
        }]),
        SessionEvent::TurnEnded,
    ];
    let adapter: std::sync::Arc<dyn HarnessAdapter> =
        std::sync::Arc::new(FakeHarnessAdapter::new(events));
    let created = adapter
        .new_session(SessionSpec {
            session: session.clone(),
            harness: wire::harness::HarnessId::GrokBuild,
            model: None,
            effort: None,
            working_dir: Some(PathBuf::from("/work/acme")),
        })
        .await?;
    assert_eq!(created, session);
    let mut rx = adapter.events(&session).await?;
    assert!(adapter.events(&session).await.is_err());
    adapter
        .prompt(&session, TurnInput::Text("go".to_string()))
        .await?;
    let mut seen = Vec::new();
    for _ in 0..5 {
        let event = rx.recv().await.ok_or("expected an event")?;
        seen.push(event);
    }
    assert_eq!(seen.len(), 5);
    adapter
        .answer_permission(&session, "r1", Decision::Allow)
        .await?;
    adapter
        .set_option(&session, SessionOption::Model("m".to_string()))
        .await?;
    Ok(())
}

#[tokio::test]
async fn terminal_mux_spawn_kill_list() -> Result<(), Box<dyn std::error::Error>> {
    let mux: std::sync::Arc<dyn TerminalMux> = std::sync::Arc::new(FakeTerminalMux::new());
    let session = SessionId::from_str("claude-code:11111111-2222-4333-8444-555555555555")?;
    let id = mux
        .spawn(
            &session,
            vec!["bash".to_string()],
            std::path::Path::new("/work/acme"),
            TermSize { cols: 80, rows: 24 },
        )
        .await?;
    let rx = mux.attach(&id).await?;
    mux.input(&id, b"ls\n".to_vec()).await?;
    mux.resize(
        &id,
        TermSize {
            cols: 120,
            rows: 40,
        },
    )
    .await?;
    let listed = mux.list().await?;
    assert!(listed.first().is_some_and(|pty| pty.running));
    mux.kill(&id).await?;
    let listed = mux.list().await?;
    assert!(listed.first().is_some_and(|pty| !pty.running));
    drop(rx);
    Ok(())
}

#[tokio::test]
async fn tool_echoes_text_argument() -> Result<(), Box<dyn std::error::Error>> {
    let tool: std::sync::Arc<dyn Tool> = std::sync::Arc::new(EchoTool::new("echo"));
    let definition = tool.definition();
    assert_eq!(definition.name.as_ref(), "echo");
    let args = serde_json::json!({"text": "ping"});
    let result = tool
        .call(
            args.as_object().cloned(),
            ToolContext {
                agent_id: Some("agent-a".to_string()),
                working_dir: None,
                session: None,
            },
        )
        .await?;
    assert!(!result.is_error.unwrap_or(false));
    let missing = tool
        .call(
            None,
            ToolContext {
                agent_id: None,
                working_dir: None,
                session: None,
            },
        )
        .await;
    assert!(missing.is_err());
    Ok(())
}

#[tokio::test]
async fn transcript_reader_reads_and_tails() -> Result<(), Box<dyn std::error::Error>> {
    let session = SessionId::from_str("codex:native-9")?;
    let early = Turn {
        role: wire::message::Role::User,
        content: "early".to_string(),
        at: DateTime::parse_from_rfc3339("2026-10-12T00:00:00Z")?.with_timezone(&Utc),
    };
    let late = Turn {
        role: wire::message::Role::Assistant,
        content: "late".to_string(),
        at: DateTime::parse_from_rfc3339("2026-10-12T01:00:00Z")?.with_timezone(&Utc),
    };
    let mut turns = BTreeMap::new();
    turns.insert(session.to_string(), vec![early.clone(), late.clone()]);
    let reader: std::sync::Arc<dyn TranscriptReader> =
        std::sync::Arc::new(FakeTranscriptReader::new(turns));
    let listed = reader
        .sessions(std::path::Path::new("/work/acme/.claude"))
        .await?;
    assert_eq!(listed, vec![session.clone()]);
    let read = reader.read(&session).await?;
    assert_eq!(read.len(), 2);
    let since = DateTime::parse_from_rfc3339("2026-10-12T00:30:00Z")?.with_timezone(&Utc);
    let tail = reader.tail(&session, since).await?;
    assert_eq!(tail, vec![late]);
    Ok(())
}
