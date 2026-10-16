use wire::capture::{CaptureBatchRequest, CaptureMessage, CaptureResult};
use wire::message::Role;

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
