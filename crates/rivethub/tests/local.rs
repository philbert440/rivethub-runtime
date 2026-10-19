#[path = "local/capture.rs"]
mod capture;
#[path = "local/consolidate.rs"]
mod consolidate;
#[path = "local/embed.rs"]
mod embed;
#[path = "local/import.rs"]
mod import;
#[path = "local/memory_http.rs"]
mod memory_http;

use std::net::SocketAddr;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use kernel::ToolContext;
use kernel::tasks::TaskStore as _;
use kernel::tool::Tool;
use mcp::client::Client;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio_util::sync::CancellationToken;
use wire::gateway::HealthzResponse;
use wire::gateway::memory::MemoryBrowseResponse;
use wire::memory::MemoryEntry;
use wire::message::Role;

const SUMMARY_ID: &str = "0199e2a0-7b3c-7a00-8000-000000000001";
const WIKI_PATCH: &str = concat!(
    r#"[{"action":"create","slug":"acmekit-deploys","title":"AcmeKit deploys","#,
    r#""summary":"AcmeKit deploys on Fridays.","aliases":[],"tags":[],"entities":[]}]"#
);
const DELEGATE_TEXT: &str = "delegate finished the ledger";

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Started {
    child: tokio::process::Child,
    logs: tokio::task::JoinHandle<std::io::Result<String>>,
}

fn signal_process(pid: u32, signal: nix::sys::signal::Signal) -> TestResult {
    let pid = i32::try_from(pid)?;
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), signal)?;
    Ok(())
}

fn terminate(pid: u32) -> TestResult {
    signal_process(pid, nix::sys::signal::Signal::SIGTERM)
}

async fn start(
    home: &std::path::Path,
    args: &[&str],
    needles: &'static [&str],
) -> TestResult<Started> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rh"))
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("TOKIO_WORKER_THREADS", "2")
        .current_dir(home)
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stderr = child.stderr.take().ok_or("missing stderr")?;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let logs = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut text = String::new();
        let mut pending: Vec<&str> = needles.to_vec();
        let mut ready = Some(ready_tx);
        while let Some(line) = lines.next_line().await? {
            if let Some(index) = pending.iter().position(|needle| line.contains(needle)) {
                pending.swap_remove(index);
            }
            if pending.is_empty()
                && let Some(sender) = ready.take()
                && sender.send(()).is_err()
            {
                return Err(std::io::Error::other("readiness listener dropped"));
            }
            text.push_str(&line);
            text.push('\n');
        }
        Ok(text)
    });
    if tokio::time::timeout(Duration::from_secs(30), ready_rx).await == Ok(Ok(())) {
        Ok(Started { child, logs })
    } else {
        if child.try_wait()?.is_none()
            && let Some(pid) = child.id()
        {
            terminate(pid)?;
        }
        let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
        Err(format!("rh local did not become ready: {text}").into())
    }
}

async fn stop(started: Started) -> TestResult<String> {
    let Started { mut child, logs } = started;
    let pid = child.id().ok_or("rh local already stopped")?;
    terminate(pid)?;
    let status = tokio::time::timeout(Duration::from_secs(30), child.wait()).await??;
    let text = tokio::time::timeout(Duration::from_secs(30), logs).await???;
    if status.success() {
        Ok(text)
    } else {
        Err(format!("rh local exit: {status}\n{text}").into())
    }
}

fn provider(home: &std::path::Path, extra: &str) -> String {
    provider_base(home, "http://127.0.0.1:1", extra)
}

fn provider_base(home: &Path, base: &str, extra: &str) -> String {
    format!(
        "runtime:\n  workspace: {home}\n  default_agent: main\nagents:\n  main:\n    provider: p\n    model: m\nproviders:\n  p:\n    type: openai-compatible\n    base_url: {base}\n    model: m\n{extra}",
        home = home.display()
    )
}

fn write_config(home: &std::path::Path, body: &str) -> TestResult {
    let state = home.join(".rivethub");
    std::fs::create_dir_all(&state)?;
    std::fs::write(state.join("config.yaml"), body)?;
    Ok(())
}

#[tokio::test]
async fn local_serves_health_and_sqlite_memory_then_stops_on_sigterm() -> TestResult {
    let home = tempfile::tempdir()?;
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = probe.local_addr()?.port();
    drop(probe);
    let port_text = port.to_string();
    let started = start(
        home.path(),
        &["local", "--no-lan", "--port", &port_text],
        &["gateway listening on "],
    )
    .await?;
    let client = reqwest::Client::builder().no_proxy().build()?;
    let base = format!("http://127.0.0.1:{port}");
    let health = client
        .get(format!("{base}/healthz"))
        .send()
        .await?
        .error_for_status()?
        .json::<HealthzResponse>()
        .await?;
    assert!(health.ok);
    let database = home.path().join(".rivethub").join("memory.sqlite");
    let marker = "local-sqlite-marker";
    {
        let store = memory_sqlite::SqliteStore::open(&database)?;
        let now = chrono::Utc::now();
        store
            .append_entries(
                vec![MemoryEntry {
                    id: None,
                    session_id: "local-e2e".to_owned(),
                    agent: "main".to_owned(),
                    channel: "test".to_owned(),
                    role: Role::User,
                    content: marker.to_owned(),
                    tool_name: None,
                    tool_args: None,
                    tool_result: None,
                    metadata: None,
                    created_at: Some(now),
                }],
                now,
            )
            .await?;
    }
    let browse = client
        .get(format!("{base}/api/memory/browse"))
        .send()
        .await?
        .error_for_status()?
        .json::<MemoryBrowseResponse>()
        .await?;
    assert!(
        browse
            .messages
            .iter()
            .any(|message| message.content == marker),
        "{browse:?}"
    );
    let text = std::fs::read_to_string(home.path().join(".rivethub/config.yaml"))?;
    assert!(text.contains("backend: sqlite"), "{text}");
    assert!(!text.contains("agents"), "{text}");
    assert!(!text.contains("providers"), "{text}");
    assert!(!text.contains("runtime"), "{text}");
    let logs = stop(started).await?;
    assert!(logs.contains("SIGTERM received — shutting down"), "{logs}");
    assert!(logs.contains("gateway stopped"), "{logs}");
    assert!(!home.path().join(".rivethub/rivethub.pid").exists());
    Ok(())
}

#[tokio::test]
async fn sqlite_builds_a_wiki_topic_from_a_seeded_summary() -> TestResult {
    let home = tempfile::tempdir()?;
    let port = free_port()?;
    let stub = model_stub().await?;
    let database = home.path().join(".rivethub").join("memory.sqlite");
    let wiki = home.path().join("wiki");
    std::fs::create_dir_all(&wiki)?;
    write_config(
        home.path(),
        &provider_base(
            home.path(),
            &format!("http://{}", stub.address),
            &format!(
                "memory:\n  backend: sqlite\n  sqlite:\n    path: {}\n    wiki_dir: {}\nden:\n  host: 127.0.0.1\n  port: {port}\n",
                database.display(),
                wiki.display()
            ),
        ),
    )?;
    seed_summary(&database)?;
    let built = wiki_topic(home.path(), &database).await;
    let closed = stub.close().await;
    built?;
    closed
}

async fn wiki_topic(home: &Path, database: &Path) -> TestResult {
    let started = start(
        home,
        &["local", "--no-lan"],
        &[
            "worker running wiki-builder",
            "wiki topic stored acmekit-deploys",
        ],
    )
    .await?;
    let title = topic_title(database);
    stop(started).await?;
    assert_eq!(title?, "AcmeKit deploys");
    Ok(())
}

#[tokio::test]
async fn sqlite_worker_roles_and_mcp_share_the_process() -> TestResult {
    let home = tempfile::tempdir()?;
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = probe.local_addr()?.port();
    drop(probe);
    let database = home.path().join(".rivethub").join("memory.sqlite");
    let socket = home.path().join("mcp.sock");
    write_config(
        home.path(),
        &provider(
            home.path(),
            &format!(
                "memory:\n  backend: sqlite\n  sqlite:\n    path: {}\n    workers: true\n    tagging: true\nden:\n  host: 127.0.0.1\n  port: {port}\ntransports:\n  mcp:\n    socket: {}\n",
                database.display(),
                socket.display()
            ),
        ),
    )?;
    let started = start(
        home.path(),
        &["local", "--no-lan"],
        &[
            "gateway listening on ",
            "worker running embedder",
            "worker running summarizer",
            "worker running categorizer",
            "mcp listening on ",
        ],
    )
    .await?;
    let logs = stop(started).await?;
    assert!(logs.contains("SIGTERM received — shutting down"), "{logs}");
    assert!(!socket.exists(), "{logs}");
    assert!(!home.path().join(".rivethub/rivethub.pid").exists());
    Ok(())
}

#[tokio::test]
async fn mcp_delegate_task_runs_to_completion() -> TestResult {
    let home = tempfile::tempdir()?;
    let gateway = free_port()?;
    let mcp_port = free_port()?;
    let stub = model_stub().await?;
    let database = home.path().join(".rivethub").join("memory.sqlite");
    write_config(
        home.path(),
        &provider_base(
            home.path(),
            &format!("http://{}", stub.address),
            &mcp_config(&database, gateway, mcp_port),
        ),
    )?;
    let ran = delegated(home.path(), &database, mcp_port).await;
    let closed = stub.close().await;
    ran?;
    closed
}

fn mcp_config(database: &Path, gateway: u16, mcp_port: u16) -> String {
    format!(
        "mesh:\n  node_name: standalone-node\nmemory:\n  backend: sqlite\n  sqlite:\n    path: {}\nden:\n  host: 127.0.0.1\n  port: {gateway}\ntransports:\n  mcp:\n    host: 127.0.0.1\n    port: {mcp_port}\n    auth_token: test-secret\n",
        database.display()
    )
}

async fn delegated(home: &Path, database: &Path, mcp_port: u16) -> TestResult {
    let started = start(
        home,
        &["local", "--no-lan"],
        &["gateway listening on ", "mcp listening on "],
    )
    .await?;
    let outcome = delegate_outcome(mcp_port).await;
    stop(started).await?;
    let outcome = outcome?;
    let (body, meta) = outcome
        .split_once("\n\n---\n")
        .ok_or("delegation meta line")?;
    assert_eq!(body, DELEGATE_TEXT);
    assert!(meta.starts_with("_Delegation [completed]: "), "{meta}");
    let id: String = rusqlite::Connection::open(database)?.query_row(
        "SELECT id FROM rh_tasks ORDER BY created_at DESC LIMIT 1",
        [],
        |row| row.get(0),
    )?;
    let store = memory_sqlite::SqliteStore::open(database)?;
    let stored = store.load(id.parse()?).await?;
    assert_eq!(stored.status, wire::task::TaskStatus::Completed);
    assert_eq!(stored.claimed_by.as_deref(), Some("standalone-node:agent"));
    assert_eq!(stored.node_affinity.as_deref(), Some("standalone-node"));
    let result = stored.result.ok_or("task result")?;
    assert_eq!(result.summary, DELEGATE_TEXT);
    assert_eq!(result.verdict, wire::task::TaskVerdict::Completed);
    Ok(())
}

fn free_port() -> TestResult<u16> {
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = probe.local_addr()?.port();
    drop(probe);
    Ok(port)
}

#[derive(serde::Serialize)]
struct EmbedTargetPayload<'a> {
    #[serde(rename = "targetTable")]
    target_table: &'a str,
    #[serde(rename = "targetId")]
    target_id: &'a str,
}

const MESSAGE_ID: &str = "0199e2a0-7b3c-7a00-8000-0000000000b1";
const CONVERSATION_ID: &str = "0199e2a0-7b3c-7a00-8000-0000000000b2";
const JOB_ID: &str = "0199e2a0-7b3c-7a00-8000-0000000000b3";

fn seed_embed_job(path: &Path) -> TestResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    migrate::sqlite(path)?;
    let conn = rusqlite::Connection::open(path)?;
    let now = "2020-01-01T00:00:00.000Z";
    conn.execute(
        "INSERT INTO ros_conversations (id, session_key, agent, created_at, updated_at)
         VALUES (?1, 'shutdown', 'rivet', ?2, ?2)",
        (CONVERSATION_ID, now),
    )?;
    conn.execute(
        "INSERT INTO ros_messages (id, conversation_id, agent, channel, role, content, created_at)
         VALUES (?1, ?2, 'rivet', 'test', 'user', 'hello ledger', ?3)",
        (MESSAGE_ID, CONVERSATION_ID, now),
    )?;
    let payload = serde_json::to_string(&wire::JsonObject::from_serialize(&EmbedTargetPayload {
        target_table: "ros_messages",
        target_id: MESSAGE_ID,
    })?)?;
    conn.execute(
        "INSERT INTO ros_jobs (
            id, task, job_key, payload, run_at, attempts, max_attempts, state, created_at, updated_at
         ) VALUES (?1, 'embed-target', ?2, ?3, ?4, 0, 5, 'queued', ?4, ?4)",
        (
            JOB_ID,
            format!("embed-ros_messages-{MESSAGE_ID}"),
            payload,
            now,
        ),
    )?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct SqliteJob {
    state: String,
    attempts: i64,
    locked_at: Option<String>,
    last_error: Option<String>,
}

fn sqlite_job(path: &Path) -> TestResult<SqliteJob> {
    let conn = rusqlite::Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    let row = conn.query_row(
        "SELECT state, attempts, locked_at, last_error FROM ros_jobs WHERE id = ?1",
        [JOB_ID],
        |row| {
            Ok(SqliteJob {
                state: row.get(0)?,
                attempts: row.get(1)?,
                locked_at: row.get(2)?,
                last_error: row.get(3)?,
            })
        },
    )?;
    Ok(row)
}

async fn blackhole() -> TestResult<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0_u8; 1024];
                loop {
                    match tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
        }
    });
    Ok((address, task))
}

async fn wait_running(database: &Path) -> Result<(), String> {
    let result = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match sqlite_job(database) {
                Ok(job) if job.state == "running" && job.attempts == 1 => return Ok(()),
                Ok(_) => tokio::task::yield_now().await,
                Err(error) => return Err(error.to_string()),
            }
        }
    })
    .await;
    match result {
        Ok(inner) => inner,
        Err(_) => Err("embed job was not claimed".to_owned()),
    }
}

#[tokio::test]
async fn local_blocked_on_a_provider_releases_the_job_on_sigint() -> TestResult {
    let home = tempfile::tempdir()?;
    let port = free_port()?;
    let (provider, hold) = blackhole().await?;
    let database = home.path().join(".rivethub").join("memory.sqlite");
    write_config(
        home.path(),
        &provider_base(
            home.path(),
            &format!("http://{provider}"),
            &format!(
                "memory:\n  backend: sqlite\n  sqlite:\n    path: {}\n    workers: true\nden:\n  host: 127.0.0.1\n  port: {port}\n",
                database.display()
            ),
        ),
    )?;
    seed_embed_job(&database)?;
    let started = match start(
        home.path(),
        &["local", "--no-lan"],
        &["gateway listening on ", "worker running embedder"],
    )
    .await
    {
        Ok(started) => started,
        Err(error) => {
            hold.abort();
            return Err(error);
        }
    };
    let claimed = wait_running(&database).await;
    let Started { mut child, logs } = started;
    if let Err(error) = claimed {
        if let Some(pid) = child.id() {
            signal_process(pid, nix::sys::signal::Signal::SIGINT)?;
        }
        let text = tokio::time::timeout(Duration::from_secs(5), logs).await???;
        hold.abort();
        return Err(format!("{error}\n{text}").into());
    }
    let pid = child.id().ok_or("rh local already stopped")?;
    let began = tokio::time::Instant::now();
    signal_process(pid, nix::sys::signal::Signal::SIGINT)?;
    let waited = tokio::time::timeout(kernel::shutdown::SHUTDOWN_GRACE, child.wait()).await;
    let elapsed = began.elapsed();
    let Ok(status) = waited else {
        child.start_kill()?;
        let text = logs.await??;
        hold.abort();
        return Err(format!("rh local still running after {elapsed:?}\n{text}").into());
    };
    let status = status?;
    let text = tokio::time::timeout(Duration::from_secs(5), logs).await???;
    hold.abort();
    assert!(
        elapsed < kernel::shutdown::SHUTDOWN_GRACE,
        "{elapsed:?}\n{text}"
    );
    assert!(status.success(), "rh local exit: {status}\n{text}");
    assert!(text.contains("SIGINT received — shutting down"), "{text}");
    assert_eq!(
        sqlite_job(&database)?,
        SqliteJob {
            state: "queued".to_owned(),
            attempts: 0,
            locked_at: None,
            last_error: None,
        }
    );
    Ok(())
}

fn seed_summary(path: &Path) -> TestResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    migrate::sqlite(path)?;
    let conn = rusqlite::Connection::open(path)?;
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    conn.execute(
        "INSERT INTO ros_summaries (id, content, created_at) VALUES (?1, ?2, ?3)",
        (SUMMARY_ID, "AcmeKit deploys on Fridays.", now),
    )?;
    Ok(())
}

fn topic_title(path: &Path) -> TestResult<String> {
    let conn = rusqlite::Connection::open(path)?;
    let title = conn.query_row(
        "SELECT title FROM ros_wiki_topics WHERE slug = 'acmekit-deploys'",
        [],
        |row| row.get(0),
    )?;
    Ok(title)
}

struct ModelStub {
    address: SocketAddr,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<std::io::Result<()>>,
}

async fn model_stub() -> TestResult<ModelStub> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let cancel = CancellationToken::new();
    let stopped = cancel.clone();
    let router = axum::Router::new().route("/v1/chat/completions", axum::routing::post(complete));
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
    });
    Ok(ModelStub {
        address,
        cancel,
        task,
    })
}

impl ModelStub {
    async fn close(self) -> TestResult {
        self.cancel.cancel();
        tokio::time::timeout(Duration::from_secs(5), self.task).await???;
        Ok(())
    }
}

async fn complete(
    body: axum::body::Bytes,
) -> Result<([(axum::http::HeaderName, &'static str); 1], String), (axum::http::StatusCode, String)>
{
    let text = std::str::from_utf8(&body)
        .map_err(|error| (axum::http::StatusCode::BAD_REQUEST, error.to_string()))?;
    let content = if text.contains("Extract durable") {
        WIKI_PATCH
    } else {
        DELEGATE_TEXT
    };
    let payload = event_stream(content).map_err(|error| {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            error.to_string(),
        )
    })?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        payload,
    ))
}

fn event_stream(content: &str) -> TestResult<String> {
    let chunk = wire::gateway::v1::CompletionChunk {
        id: "c".to_owned(),
        created: 0,
        model: "m".to_owned(),
        choices: vec![wire::gateway::v1::ChunkChoice {
            index: 0,
            delta: wire::gateway::v1::CompletionDelta {
                content: Some(content.to_owned()),
                ..wire::gateway::v1::CompletionDelta::default()
            },
            finish_reason: None,
        }],
        usage: None,
    };
    Ok(format!(
        "data: {}\n\ndata: [DONE]\n\n",
        serde_json::to_string(&chunk)?
    ))
}

async fn delegate_outcome(port: u16) -> TestResult<String> {
    let shutdown = CancellationToken::new();
    let http = reqwest::Client::builder().no_proxy().build()?;
    let config =
        StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
            .auth_header("test-secret");
    let client = Client::connect(
        StreamableHttpClientTransport::with_client(http, config),
        shutdown.clone(),
    )
    .await?;
    let tools = client.tools().await?;
    let outcome = call_delegate(&tools).await;
    let closed = client.close().await;
    shutdown.cancel();
    closed?;
    outcome
}

async fn call_delegate(tools: &[Arc<dyn Tool>]) -> TestResult<String> {
    let tool = tools
        .iter()
        .find(|tool| tool.definition().name == "delegate_task")
        .ok_or("delegate_task")?;
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        tool.call(
            Some(serde_json::from_str(
                r#"{"to_agent":"main","task":"finish the ledger"}"#,
            )?),
            ToolContext {
                agent_id: None,
                working_dir: None,
                session: None,
            },
        ),
    )
    .await??;
    let [block] = result.content.as_slice() else {
        return Err("one result block".into());
    };
    let Some(text) = block.as_text() else {
        return Err("text outcome".into());
    };
    Ok(text.text.clone())
}
