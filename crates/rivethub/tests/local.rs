use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use wire::gateway::HealthzResponse;
use wire::gateway::memory::MemoryBrowseResponse;
use wire::memory::MemoryEntry;
use wire::message::Role;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct Started {
    child: tokio::process::Child,
    logs: tokio::task::JoinHandle<std::io::Result<String>>,
}

fn terminate(pid: u32) -> TestResult {
    let pid = i32::try_from(pid)?;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    )?;
    Ok(())
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
    format!(
        "runtime:\n  workspace: {home}\n  default_agent: main\nagents:\n  main:\n    provider: p\n    model: m\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:1\n    model: m\n{extra}",
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
async fn wiki_dir_refuses_startup() -> TestResult {
    let home = tempfile::tempdir()?;
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = probe.local_addr()?.port();
    drop(probe);
    let database = home.path().join(".rivethub").join("memory.sqlite");
    let wiki = home.path().join("wiki");
    std::fs::create_dir_all(&wiki)?;
    write_config(
        home.path(),
        &provider(
            home.path(),
            &format!(
                "memory:\n  backend: sqlite\n  sqlite:\n    path: {}\n    wiki_dir: {}\nden:\n  host: 127.0.0.1\n  port: {port}\n",
                database.display(),
                wiki.display()
            ),
        ),
    )?;
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(env!("CARGO_BIN_EXE_rh"))
            .args(["local", "--no-lan"])
            .env_clear()
            .env("HOME", home.path())
            .env("TOKIO_WORKER_THREADS", "2")
            .current_dir(home.path())
            .output(),
    )
    .await??;
    let text = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("wiki builder"), "{text}");
    assert!(!home.path().join(".rivethub/rivethub.pid").exists());
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
