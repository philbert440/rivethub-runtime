use wire::capture::{CaptureBatchRequest, CaptureMessage, CaptureResult};
use wire::message::Role;

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt as _;

use super::{TestResult, free_port, start, stop};

const SESSION: &str = "capture-resend";

fn message(event_id: &str, role: Role, content: &str) -> CaptureMessage {
    CaptureMessage {
        event_id: event_id.to_owned(),
        role,
        content: content.to_owned(),
        tool_name: None,
        tool_args: None,
        tool_result: None,
        metadata: None,
        created_at: None,
    }
}

fn batch(title: &str) -> CaptureBatchRequest {
    CaptureBatchRequest {
        session_key: SESSION.to_owned(),
        agent: "claude-code".to_owned(),
        channel: Some("claude-code".to_owned()),
        title: Some(title.to_owned()),
        settings: None,
        task_id: None,
        finalize: None,
        created_at: None,
        updated_at: None,
        messages: vec![
            message("evt-1", Role::User, "first"),
            message("evt-2", Role::Assistant, "second"),
        ],
    }
}

#[tokio::test]
async fn resent_capture_batches_skip_stored_events_and_keep_the_title() -> TestResult {
    let home = tempfile::tempdir()?;
    let port = free_port()?;
    let port_text = port.to_string();
    let started = start(
        home.path(),
        &["local", "--no-lan", "--port", &port_text],
        &["gateway listening on "],
    )
    .await?;
    let client = reqwest::Client::builder().no_proxy().build()?;
    let url = format!("http://127.0.0.1:{port}/api/capture");
    let mut results = Vec::new();
    for title in ["Stop", "SessionEnd"] {
        let response = client.post(&url).json(&batch(title)).send().await;
        results.push(match response {
            Ok(response) => (response.status(), response.text().await),
            Err(error) => {
                stop(started).await?;
                return Err(error.into());
            }
        });
    }
    stop(started).await?;
    let mut counts = Vec::new();
    for (status, body) in results {
        let body = body?;
        assert_eq!(status, reqwest::StatusCode::OK, "{body}");
        let result: CaptureResult = serde_json::from_str(&body)?;
        counts.push((result.inserted, result.skipped));
    }
    assert_eq!(counts, vec![(2, 0), (0, 2)]);
    let database = home.path().join(".rivethub").join("memory.sqlite");
    let conn = rusqlite::Connection::open(&database)?;
    let rows: i64 = conn.query_row(
        "SELECT count(*) FROM ros_messages m JOIN ros_conversations c ON c.id = m.conversation_id
          WHERE c.session_key = ?1",
        [SESSION],
        |row| row.get(0),
    )?;
    assert_eq!(rows, 2);
    let title: Option<String> = conn.query_row(
        "SELECT title FROM ros_conversations WHERE session_key = ?1",
        [SESSION],
        |row| row.get(0),
    )?;
    assert_eq!(title.as_deref(), Some("SessionEnd"));
    Ok(())
}

const HOOK_SESSION: &str = "44444444-4444-4444-4444-444444444444";

#[derive(serde::Serialize)]
struct PromptHook<'a> {
    session_id: &'a str,
    cwd: &'a str,
    hook_event_name: &'a str,
    prompt: &'a str,
}

async fn run_hook(home: &std::path::Path, payload: &str) -> TestResult<std::process::Output> {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_rh"))
        .args(["capture", "hook"])
        .env_clear()
        .env("HOME", home)
        .env("RIVETHUB_CAPTURE_REDACTION", "1")
        .current_dir(home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("missing stdin")?;
    stdin.write_all(payload.as_bytes()).await?;
    drop(stdin);
    Ok(tokio::time::timeout(Duration::from_secs(30), child.wait_with_output()).await??)
}

#[tokio::test]
async fn the_capture_hook_command_posts_a_prompt_to_the_local_gateway() -> TestResult {
    let home = tempfile::tempdir()?;
    let port_text = free_port()?.to_string();
    let started = start(
        home.path(),
        &["local", "--no-lan", "--port", &port_text],
        &["gateway listening on "],
    )
    .await?;
    let payload = serde_json::to_string(&PromptHook {
        session_id: HOOK_SESSION,
        cwd: "/work/demo",
        hook_event_name: "UserPromptSubmit",
        prompt: "deploy with Bearer abcdefghijklmnop1234",
    })?;
    let output = run_hook(home.path(), &payload).await;
    stop(started).await?;
    let output = output?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(output.status.success(), "{stderr}");
    assert_eq!(String::from_utf8(output.stdout)?, "");
    let conn = rusqlite::Connection::open(home.path().join(".rivethub").join("memory.sqlite"))?;
    let content: String = conn.query_row(
        "SELECT m.content FROM ros_messages m JOIN ros_conversations c ON c.id = m.conversation_id
          WHERE c.session_key = ?1",
        [format!("claude-code:{HOOK_SESSION}")],
        |row| row.get(0),
    )?;
    assert_eq!(content, "deploy with [REDACTED:bearer]");
    let log = std::fs::read_to_string(home.path().join(".rivethub").join("claude-capture.log"))?;
    assert!(log.contains("UserPromptSubmit"), "{log}");
    Ok(())
}
