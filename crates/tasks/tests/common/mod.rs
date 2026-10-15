use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::Path;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::routing::get;
use axum::routing::post;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use tasks::Directory;
use tasks::Engine;
use tasks::LocalAgent;
use tasks::NewTask;
use tasks::Settings;
use tasks::TaskOrigin;
use testkit::TaskMemory;
use tokio::sync::Notify;
use tokio::sync::mpsc;
use url::Url;
use wire::gateway::sessions::MessageUsage;
use wire::mesh::MeshNode;
use wire::mesh::MeshNodeStatus;
use wire::task::TaskBudget;
use wire::task::TaskCreateRequest;
use wire::task::TaskExecutorKind;
use wire::task::TaskSpec;

#[derive(Debug)]
pub(crate) struct Recorded {
    pub(crate) path: String,
    pub(crate) body: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct Capture {
    posts: mpsc::UnboundedSender<Recorded>,
}

pub(crate) struct Script {
    pub(crate) text: &'static str,
    pub(crate) model: Option<&'static str>,
    pub(crate) prompt_tokens: Option<u64>,
    pub(crate) completion_tokens: Option<u64>,
    pub(crate) hang: bool,
    pub(crate) hold: Option<Arc<Notify>>,
    pub(crate) gateway_timeout: bool,
}

impl Script {
    pub(crate) fn reply(text: &'static str) -> Self {
        Self {
            text,
            model: None,
            prompt_tokens: None,
            completion_tokens: None,
            hang: false,
            hold: None,
            gateway_timeout: false,
        }
    }
}

pub(crate) fn base_url(addr: std::net::SocketAddr) -> Url {
    Url::parse(&format!("http://{addr}/")).expect("base url")
}

pub(crate) fn plain_agent() -> LocalAgent {
    LocalAgent {
        model: None,
        provider: None,
        effort: None,
        harness: None,
        system_prompt: None,
        working_dir: None,
    }
}

pub(crate) fn directory(base: Url) -> Directory {
    let mut directory = Directory::new("local", base).expect("directory");
    directory.local_agent("worker", plain_agent());
    directory
}

pub(crate) fn settings(
    root: Option<PathBuf>,
    pricing: BTreeMap<String, tasks::TokenRate>,
) -> Settings {
    Settings {
        node: "local".to_string(),
        worker: "worker-1".to_string(),
        files_root: root,
        pricing,
        api_token: None,
    }
}

pub(crate) fn engine_with(
    directory: Directory,
    settings: Settings,
) -> (Engine<TaskMemory, TaskMemory>, TaskMemory) {
    let memory = TaskMemory::new();
    let engine = Engine::new(memory.clone(), memory.clone(), directory, settings).expect("engine");
    (engine, memory)
}

pub(crate) fn engine(base: Url) -> Engine<TaskMemory, TaskMemory> {
    let (engine, _) = engine_with(directory(base), settings(None, BTreeMap::new()));
    engine
}

pub(crate) fn request(goal: &str) -> TaskCreateRequest {
    TaskCreateRequest {
        goal: goal.to_string(),
        agent_id: "worker".to_string(),
        executor: Some(TaskExecutorKind::ChatLoop),
        executor_target: None,
        requested_by: None,
        parent_task_id: None,
        chain_depth: None,
        node_affinity: None,
        spec: None,
        budget: None,
        context_refs: None,
        acceptance_criteria: None,
    }
}

pub(crate) fn task(goal: &str) -> NewTask {
    NewTask {
        request: request(goal),
        origin: TaskOrigin::Chat,
    }
}

pub(crate) fn spec(session: &str, append: Option<&str>, working: Option<&str>) -> TaskSpec {
    TaskSpec {
        task_id: String::new(),
        agent_id: "worker".to_string(),
        goal: "ship it".to_string(),
        resolved_context: Vec::new(),
        acceptance_criteria: Vec::new(),
        budget: TaskBudget::default(),
        tools: None,
        working_dir: working.map(ToString::to_string),
        model: None,
        effort: None,
        system_prompt_append: append.map(ToString::to_string),
        isolation: None,
        prompt_mode: None,
        exclude_tools: None,
        resume_message: None,
        session: session.to_string(),
        permission_decisions: None,
    }
}

pub(crate) async fn chat_stub(
    script: Script,
) -> (testkit::stub::Stub, mpsc::UnboundedReceiver<Recorded>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/sessions/{session}/messages", post(chat))
        .with_state((Capture { posts: tx }, Arc::new(script)));
    let stub = testkit::stub::stub_server(app).await.expect("stub");
    (stub, rx)
}

pub(crate) async fn redirect_stub(kind: RedirectKind) -> (testkit::stub::Stub, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/healthz",
            get({
                let hits = Arc::clone(&hits);
                move || redirect_health(kind, Arc::clone(&hits))
            }),
        )
        .route("/mid", get(mid))
        .route("/ok", get(healthz));
    let stub = testkit::stub::stub_server(app).await.expect("stub");
    (stub, hits)
}

#[derive(Clone, Copy)]
pub(crate) enum RedirectKind {
    OneHop,
    TwoHop,
    SixHop,
    CrossHost,
}

#[expect(
    clippy::unused_async,
    reason = "axum serves this closure as a handler only when the call returns a future"
)]
async fn redirect_health(kind: RedirectKind, hits: Arc<AtomicUsize>) -> axum::response::Response {
    hits.fetch_add(1, Ordering::SeqCst);
    let location = match kind {
        RedirectKind::OneHop => "/ok",
        RedirectKind::TwoHop => "/mid",
        RedirectKind::SixHop => "/healthz",
        RedirectKind::CrossHost => "http://example.invalid/ok",
    };
    found(location)
}

async fn mid() -> axum::response::Response {
    found("/ok")
}

fn found(location: &'static str) -> axum::response::Response {
    axum::response::Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(axum::body::Body::empty())
        .expect("redirect")
}

async fn healthz() -> axum::response::Response {
    json_response(
        StatusCode::OK,
        &HealthDoc {
            ok: true,
            sessions: 0,
            name: "stub",
            node: "local",
        },
    )
}

async fn chat(
    State((capture, script)): State<(Capture, Arc<Script>)>,
    Path(session): Path<String>,
    axum::extract::RawQuery(query): axum::extract::RawQuery,
    body: Bytes,
) -> axum::response::Response {
    let path = match query.as_deref() {
        Some(query) if !query.is_empty() => format!("/api/sessions/{session}/messages?{query}"),
        Some(_) | None => format!("/api/sessions/{session}/messages"),
    };
    send(
        &capture,
        Recorded {
            path,
            body: body.to_vec(),
        },
    );
    if script.hang {
        std::future::pending::<()>().await;
    }
    if let Some(hold) = &script.hold {
        hold.notified().await;
    }
    if script.gateway_timeout {
        return json_response(
            StatusCode::GATEWAY_TIMEOUT,
            &DeadlineDoc {
                error: "no reply before deadline",
            },
        );
    }
    json_response(StatusCode::OK, &reply(&script))
}

fn send(capture: &Capture, recorded: Recorded) {
    capture.posts.send(recorded).expect("record");
}

fn reply(script: &Script) -> ReplyDoc {
    let usage = match (script.prompt_tokens, script.completion_tokens) {
        (Some(prompt), Some(completion)) => Some(MessageUsage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            cached_tokens: 0,
        }),
        (None, None | Some(_)) | (Some(_), None) => None,
    };
    ReplyDoc {
        message: message_doc(script.text, script.model, usage, None),
    }
}

fn message_doc(
    text: &'static str,
    model: Option<&'static str>,
    usage: Option<MessageUsage>,
    thinking: Option<bool>,
) -> MessageDoc {
    MessageDoc {
        id: "m1",
        session_id: "s1",
        role: "assistant",
        text,
        ts: 1_760_000_001_000,
        usage,
        model,
        duration_ms: None,
        thinking,
        tools: None,
    }
}

fn json_response(status: StatusCode, body: &impl Serialize) -> axum::response::Response {
    let bytes = serde_json::to_vec(body).expect("json");
    axum::response::Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(bytes))
        .expect("response")
}

#[derive(Serialize)]
struct HealthDoc {
    ok: bool,
    sessions: u64,
    name: &'static str,
    node: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplyDoc {
    message: MessageDoc,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageDoc {
    id: &'static str,
    session_id: &'static str,
    role: &'static str,
    text: &'static str,
    ts: i64,
    usage: Option<MessageUsage>,
    model: Option<&'static str>,
    duration_ms: Option<u64>,
    thinking: Option<bool>,
    tools: Option<Vec<String>>,
}

#[derive(Serialize)]
struct DeadlineDoc {
    error: &'static str,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PostedChat {
    pub(crate) text: String,
    #[serde(default)]
    pub(crate) user_id: Option<String>,
    #[serde(default)]
    pub(crate) agent: Option<String>,
    #[serde(default)]
    pub(crate) system_prompt: Option<String>,
}

pub(crate) fn posted(body: &[u8]) -> PostedChat {
    serde_json::from_slice(body).expect("posted chat")
}

pub(crate) fn next_body(rx: &mut mpsc::UnboundedReceiver<Recorded>) -> Recorded {
    rx.try_recv().expect("recorded request")
}

pub(crate) fn quiet(rx: &mut mpsc::UnboundedReceiver<Recorded>) {
    assert!(rx.try_recv().is_err());
}

pub(crate) fn deadline() -> Duration {
    Duration::from_secs(2)
}

pub(crate) fn node(
    id: &str,
    host: &str,
    port: u16,
    agents: &[&str],
    status: MeshNodeStatus,
) -> MeshNode {
    let stamp = DateTime::<Utc>::from_timestamp_millis(1_760_000_001_000).expect("stamp");
    MeshNode {
        id: id.to_string(),
        name: id.to_string(),
        role: None,
        agents: agents.iter().map(|agent| (*agent).to_string()).collect(),
        host: host.to_string(),
        port,
        providers: Vec::new(),
        models: Vec::new(),
        capabilities: Vec::new(),
        status,
        last_seen: stamp,
        registered_at: stamp,
        version: "1".to_string(),
        metadata: None,
        ssh_user: None,
        install_root: None,
        platform: None,
        extra: wire::Extra::default(),
    }
}
