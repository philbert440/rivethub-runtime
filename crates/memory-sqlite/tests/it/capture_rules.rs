use crate::common;

use common::TestResult;
use common::batch;
use common::fresh_store;
use common::instant;
use common::table;
use kernel::memory::CaptureBatchRequest;
use testkit::Json;
use wire::message::Role;

async fn stored_updated_at(store: &memory_sqlite::SqliteStore) -> TestResult<String> {
    let rows = table(store, "ros_conversations").await?;
    let row = rows.first().ok_or("one conversation")?;
    Ok(row
        .get("updated_at")
        .and_then(testkit::Json::as_str)
        .ok_or("updated_at text")?
        .to_owned())
}

#[tokio::test]
async fn updated_at_only_moves_forward() -> TestResult {
    let fresh = fresh_store()?;
    let now = instant(common::RECORDED_START)?;
    let stamped = |updated_at: &str| -> TestResult<CaptureBatchRequest> {
        let mut capture = batch("old-task", "rivet", "cli");
        capture.updated_at = Some(instant(updated_at)?);
        Ok(capture)
    };
    fresh
        .store
        .capture(
            stamped("2025-03-01T01:00:00+01:00")?,
            &testkit::owner()?,
            None,
            now,
        )
        .await?;
    fresh
        .store
        .capture(
            stamped("2024-06-01T00:00:00Z")?,
            &testkit::owner()?,
            None,
            now,
        )
        .await?;
    assert_eq!(
        stored_updated_at(&fresh.store).await?,
        "2025-03-01T00:00:00.000Z"
    );
    fresh
        .store
        .capture(
            batch("old-task", "rivet", "cli"),
            &testkit::owner()?,
            None,
            now,
        )
        .await?;
    assert_eq!(
        stored_updated_at(&fresh.store).await?,
        common::RECORDED_START
    );
    Ok(())
}

fn tool_call(result: Option<&str>, metadata: &str) -> TestResult<CaptureBatchRequest> {
    let mut capture = batch("cli:tool", "rivet", "cli");
    let mut call = common::message("cowork:s:tool:tu1", Role::Tool, "[tool call] Bash");
    call.tool_name = Some("Bash".to_owned());
    call.tool_args = Some(common::object(r#"{"command":"ls"}"#)?);
    call.tool_result = result.map(str::to_owned);
    call.metadata = Some(common::object(metadata)?);
    capture.messages.push(call);
    Ok(capture)
}

async fn job_count(store: &memory_sqlite::SqliteStore) -> TestResult<usize> {
    Ok(table(store, "ros_jobs").await?.len())
}

fn cell_str<'a>(row: &'a Json, key: &str) -> TestResult<&'a str> {
    row.get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("{key} text").into())
}

async fn only_job(store: &memory_sqlite::SqliteStore) -> TestResult<Json> {
    let mut rows = table(store, "ros_jobs").await?;
    let Some(row) = rows.pop() else {
        return Err("missing job".into());
    };
    if !rows.is_empty() {
        return Err(format!("expected one job, found {}", rows.len().saturating_add(1)).into());
    }
    Ok(row)
}

#[tokio::test]
async fn a_filled_tool_result_is_queued_again() -> TestResult {
    let fresh = fresh_store()?;
    fresh.store.arm_embed();
    let now = instant(common::RECORDED_START)?;
    fresh
        .store
        .capture(
            tool_call(None, r#"{"tool_use_id":"tu1","pending_result":true}"#)?,
            &testkit::owner()?,
            None,
            now,
        )
        .await?;
    let first = only_job(&fresh.store).await?;
    fresh
        .store
        .complete_job(cell_str(&first, "id")?.to_owned())
        .await?;
    assert_eq!(job_count(&fresh.store).await?, 0);
    fresh
        .store
        .capture(
            tool_call(
                Some("deployed build 42"),
                r#"{"tool_use_id":"tu1","pending_result":false}"#,
            )?,
            &testkit::owner()?,
            None,
            now,
        )
        .await?;
    let again = only_job(&fresh.store).await?;
    assert_eq!(cell_str(&again, "state")?, "queued");
    assert_eq!(again.get("attempts").and_then(Json::as_i64), Some(0));
    assert!(cell_str(&again, "job_key")?.starts_with("embed-ros_messages-"));
    let row = table(&fresh.store, "ros_messages")
        .await?
        .into_iter()
        .next()
        .ok_or("message")?;
    assert_eq!(
        row.get("tool_result").and_then(Json::as_str),
        Some("deployed build 42")
    );
    Ok(())
}

#[tokio::test]
async fn a_queued_tool_job_stays_when_the_result_arrives() -> TestResult {
    let fresh = fresh_store()?;
    fresh.store.arm_embed();
    let now = instant(common::RECORDED_START)?;
    fresh
        .store
        .capture(
            tool_call(None, r#"{"tool_use_id":"tu1","pending_result":true}"#)?,
            &testkit::owner()?,
            None,
            now,
        )
        .await?;
    fresh
        .store
        .capture(
            tool_call(
                Some("deployed build 42"),
                r#"{"tool_use_id":"tu1","pending_result":false}"#,
            )?,
            &testkit::owner()?,
            None,
            now,
        )
        .await?;
    assert_eq!(job_count(&fresh.store).await?, 1);
    let row = table(&fresh.store, "ros_messages")
        .await?
        .into_iter()
        .next()
        .ok_or("message")?;
    assert_eq!(
        row.get("tool_result").and_then(testkit::Json::as_str),
        Some("deployed build 42")
    );
    let job = table(&fresh.store, "ros_jobs")
        .await?
        .into_iter()
        .next()
        .ok_or("job")?;
    assert_eq!(
        job.get("state").and_then(testkit::Json::as_str),
        Some("queued")
    );
    assert_eq!(job.get("attempts").and_then(testkit::Json::as_i64), Some(0));
    Ok(())
}

async fn stored_title(store: &memory_sqlite::SqliteStore) -> TestResult<Option<String>> {
    let rows = table(store, "ros_conversations").await?;
    let row = rows.first().ok_or("one conversation")?;
    Ok(row
        .get("title")
        .and_then(testkit::Json::as_str)
        .map(str::to_owned))
}

#[tokio::test]
async fn a_placeholder_title_never_replaces_the_person_s_words() -> TestResult {
    let fresh = fresh_store()?;
    let now = instant(common::RECORDED_START)?;
    let titled = |title: &str| {
        let mut capture = batch("cli:titled", "rivet", "cli");
        capture.title = Some(title.to_owned());
        capture
    };
    fresh
        .store
        .capture(titled("Kimi Code session"), &testkit::owner()?, None, now)
        .await?;
    assert_eq!(stored_title(&fresh.store).await?, None);
    fresh
        .store
        .capture(
            titled("<user_instructions>be brief</user_instructions>\nport the parser"),
            &testkit::owner()?,
            None,
            now,
        )
        .await?;
    assert_eq!(
        stored_title(&fresh.store).await?.as_deref(),
        Some("port the parser")
    );
    fresh
        .store
        .capture(titled("Kimi Code session"), &testkit::owner()?, None, now)
        .await?;
    fresh
        .store
        .capture(
            titled("<recommended_plugins> Here is a list"),
            &testkit::owner()?,
            None,
            now,
        )
        .await?;
    assert_eq!(
        stored_title(&fresh.store).await?.as_deref(),
        Some("port the parser")
    );
    Ok(())
}
