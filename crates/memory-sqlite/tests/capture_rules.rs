#[expect(
    dead_code,
    reason = "integration binaries share fixtures and use different subsets"
)]
mod common;

use common::TestResult;
use common::batch;
use common::fresh_store;
use common::instant;
use common::table;
use testkit::Json;
use wire::capture::CaptureBatchRequest;
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
    let fresh = fresh_store(None)?;
    let now = instant(common::RECORDED_START)?;
    let stamped = |updated_at: &str| -> TestResult<CaptureBatchRequest> {
        let mut capture = batch("old-task", "rivet", "cli");
        capture.updated_at = Some(instant(updated_at)?);
        Ok(capture)
    };
    fresh
        .store
        .capture(stamped("2025-03-01T01:00:00+01:00")?, now)
        .await?;
    fresh
        .store
        .capture(stamped("2024-06-01T00:00:00Z")?, now)
        .await?;
    assert_eq!(
        stored_updated_at(&fresh.store).await?,
        "2025-03-01T00:00:00.000Z"
    );
    fresh
        .store
        .capture(batch("old-task", "rivet", "cli"), now)
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
    let fresh = fresh_store(None)?;
    fresh.store.arm_embed();
    let now = instant(common::RECORDED_START)?;
    fresh
        .store
        .capture(
            tool_call(None, r#"{"tool_use_id":"tu1","pending_result":true}"#)?,
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
    let fresh = fresh_store(None)?;
    fresh.store.arm_embed();
    let now = instant(common::RECORDED_START)?;
    fresh
        .store
        .capture(
            tool_call(None, r#"{"tool_use_id":"tu1","pending_result":true}"#)?,
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

#[tokio::test]
async fn the_project_rule_reuses_the_taxonomy() -> TestResult {
    let fresh = fresh_store(None)?;
    common::execute(
        &fresh.store,
        "INSERT INTO ros_tag_taxonomy (key, value, display, aliases, state, created_at, updated_at) \
         VALUES ('project', 'Widget Hub', 'Widget Hub', '[\"Widgets\"]', 'accepted', \
                 '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z'), \
                ('project', 'scratch', '', '[]', 'rejected', \
                 '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z');",
    )
    .await?;
    let mut applied = Vec::new();
    for (id, value, display) in [
        ("alias", "widgets", "widgets"),
        ("canonical", "widget-hub", "widget hub"),
        ("rejected", "scratch", "Scratch"),
        ("fresh", "gizmo", "gizmo"),
    ] {
        applied.push(
            fresh
                .store
                .apply_project_rule(id.to_owned(), value.to_owned(), display.to_owned())
                .await?,
        );
    }
    assert_eq!(applied, vec![1, 1, 0, 1]);
    let rows = fresh
        .store
        .with_conn(|conn| {
            let mut statement = conn
                .prepare(
                    "SELECT entity_id, value, display FROM ros_tags \
                     WHERE source = 'rule' ORDER BY entity_id",
                )
                .map_err(common::seed)?;
            let found = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(common::seed)?
                .collect::<Result<Vec<(String, String, String)>, _>>()
                .map_err(common::seed)?;
            Ok(found)
        })
        .await?;
    let expected = [
        ("alias", "Widget Hub", "Widget Hub"),
        ("canonical", "Widget Hub", "Widget Hub"),
        ("fresh", "gizmo", "gizmo"),
    ]
    .map(|(id, value, display)| (id.to_owned(), value.to_owned(), display.to_owned()));
    assert_eq!(rows, expected);
    Ok(())
}
