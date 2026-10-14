mod queue;
pub use queue::FakeJobQueue;

use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicI64, Ordering};

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
use kernel::model::Embedding;
use kernel::model::ModelChunk;
use kernel::model::ModelProvider;
use kernel::model::ModelProviderError;
use kernel::model::ModelRequest;
use kernel::model::ModelStreamItem;
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
use wire::harness::ApprovalDecision;
use wire::session_id::SessionId;
use wire::stream::LlmResponse;

#[derive(Debug, Clone)]
pub enum Script {
    Chat(LlmResponse),
    Stream(Vec<ModelChunk>),
    Embed(Vec<Embedding>),
    EmbedHttpStatus(u16),
    EmbedNetwork,
    EmbedHang,
    Models(Vec<String>),
}

#[derive(Debug, Default)]
pub struct FakeProvider {
    script: Mutex<Vec<Script>>,
    fnv_dims: Mutex<Option<usize>>,
    embed_requests: Mutex<Vec<Vec<String>>>,
    chat_requests: Mutex<Vec<ModelRequest>>,
    http_status: Mutex<Option<u16>>,
}

impl FakeProvider {
    pub fn new(script: Vec<Script>) -> Self {
        Self {
            script: Mutex::new(script),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn fnv_embeddings(width: usize) -> Self {
        Self {
            fnv_dims: Mutex::new(Some(width)),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn http_errors(status: u16) -> Self {
        Self {
            http_status: Mutex::new(Some(status)),
            ..Self::default()
        }
    }

    pub async fn use_fnv(&self, width: usize) {
        *self.http_status.lock().await = None;
        *self.fnv_dims.lock().await = Some(width);
    }

    pub async fn chat_requests(&self) -> Vec<ModelRequest> {
        self.chat_requests.lock().await.clone()
    }

    pub async fn embed_requests(&self) -> Vec<Vec<String>> {
        self.embed_requests.lock().await.clone()
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

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn fnv_token_vector(text: &str, dims: usize) -> Embedding {
    let mut vector = vec![0.0_f32; dims];
    for token in text
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
    {
        let index = fnv1a32(token.as_bytes()) as usize % dims;
        #[expect(
            clippy::indexing_slicing,
            reason = "the index is a hash modulo the vector width"
        )]
        {
            vector[index] += 1.0;
        }
    }
    Embedding { vector }
}

fn embed_http(status: u16) -> ModelProviderError {
    ModelProviderError::Request {
        message: format!("http {status}"),
        source: Box::new(std::io::Error::other(format!(
            "embed endpoint answered http {status}"
        ))),
    }
}

fn take_embed(script: &Script) -> Option<Vec<Embedding>> {
    match script {
        Script::Embed(vectors) => Some(vectors.clone()),
        Script::Chat(_)
        | Script::Stream(_)
        | Script::EmbedHttpStatus(_)
        | Script::EmbedNetwork
        | Script::EmbedHang
        | Script::Models(_) => None,
    }
}

fn take_http_status(script: &Script) -> Option<u16> {
    match script {
        Script::EmbedHttpStatus(status) => Some(*status),
        Script::Chat(_)
        | Script::Stream(_)
        | Script::Embed(_)
        | Script::EmbedNetwork
        | Script::EmbedHang
        | Script::Models(_) => None,
    }
}

fn take_network(script: &Script) -> Option<()> {
    match script {
        Script::EmbedNetwork => Some(()),
        Script::Chat(_)
        | Script::Stream(_)
        | Script::Embed(_)
        | Script::EmbedHttpStatus(_)
        | Script::EmbedHang
        | Script::Models(_) => None,
    }
}

fn take_hang(script: &Script) -> Option<()> {
    match script {
        Script::EmbedHang => Some(()),
        Script::Chat(_)
        | Script::Stream(_)
        | Script::Embed(_)
        | Script::EmbedHttpStatus(_)
        | Script::EmbedNetwork
        | Script::Models(_) => None,
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
    async fn chat(&self, request: ModelRequest) -> Result<LlmResponse, ModelProviderError> {
        self.chat_requests.lock().await.push(request);
        Ok(self
            .scripted(|s| match s {
                Script::Chat(response) => Some(response.clone()),
                Script::Stream(_)
                | Script::Embed(_)
                | Script::EmbedHttpStatus(_)
                | Script::EmbedNetwork
                | Script::EmbedHang
                | Script::Models(_) => None,
            })
            .await
            .unwrap_or_else(default_response))
    }

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> Result<Receiver<ModelStreamItem>, ModelProviderError> {
        let chunks = self
            .scripted(|s| match s {
                Script::Stream(chunks) => Some(chunks.clone()),
                Script::Chat(_)
                | Script::Embed(_)
                | Script::EmbedHttpStatus(_)
                | Script::EmbedNetwork
                | Script::EmbedHang
                | Script::Models(_) => None,
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
            drop(tx.try_send(Ok(chunk)));
        }
        Ok(rx)
    }

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        self.embed_requests.lock().await.push(texts.clone());
        let sticky = *self.http_status.lock().await;
        if let Some(status) = sticky {
            return Err(embed_http(status));
        }
        if let Some(chosen) = self.scripted(take_embed).await {
            return Ok(chosen);
        }
        if let Some(status) = self.scripted(take_http_status).await {
            return Err(embed_http(status));
        }
        if self.scripted(take_network).await.is_some() {
            return Err(ModelProviderError::Request {
                message: "connection refused".to_owned(),
                source: Box::new(std::io::Error::other("connection refused")),
            });
        }
        if self.scripted(take_hang).await.is_some() {
            std::future::pending::<()>().await;
        }
        let dims = *self.fnv_dims.lock().await;
        Ok(texts
            .into_iter()
            .map(|text| match dims {
                Some(width) => fnv_token_vector(&text, width),
                None => Embedding {
                    vector: vec![f32::from(u16::try_from(text.len()).unwrap_or(u16::MAX))],
                },
            })
            .collect())
    }

    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError> {
        Ok(self
            .scripted(|s| match s {
                Script::Models(models) => Some(models.clone()),
                Script::Chat(_)
                | Script::Stream(_)
                | Script::Embed(_)
                | Script::EmbedHttpStatus(_)
                | Script::EmbedNetwork
                | Script::EmbedHang => None,
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
    offerings: parking_lot::Mutex<Vec<wire::harness::HarnessOffering>>,
}

impl FakeHarness {
    pub fn new(events: Vec<SessionEvent>) -> Self {
        Self {
            queued: Mutex::new(events),
            sessions: Mutex::new(BTreeMap::new()),
            offerings: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn set_offerings(&self, offerings: Vec<wire::harness::HarnessOffering>) {
        *self.offerings.lock() = offerings;
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
            drop(tx.send(event).await);
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

    async fn offerings(&self) -> Result<Vec<wire::harness::HarnessOffering>, HarnessAdapterError> {
        Ok(self.offerings.lock().clone())
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
