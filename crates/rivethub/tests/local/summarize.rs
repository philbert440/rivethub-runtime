use std::path::Path;
use std::time::Duration;

use rusqlite::OptionalExtension as _;

use super::{
    DELEGATE_TEXT, TestResult, free_port, model_stub, provider_base, start, stop, write_config,
};

const CONVERSATION: &str = "0199e2a0-7b3c-7a00-8000-0000000000e1";
const IMPORTED: &str = "0199e2a07b3c7a0080000000000000e2";
const IMPORTED_CANONICAL: &str = "0199e2a0-7b3c-7a00-8000-0000000000e2";
const MESSAGES: u32 = 20;
const IDLE_TICK: Duration = Duration::from_mins(5);

fn stamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn seed_idle_conversation(path: &Path, conversation: &str, separator: &str) -> TestResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    migrate::sqlite(path)?;
    let conn = rusqlite::Connection::open(path)?;
    let started = chrono::Utc::now() - chrono::TimeDelta::hours(2);
    conn.execute(
        "INSERT INTO ros_conversations (id, session_key, agent, created_at, updated_at)
         VALUES (?1, 'idle-session', 'rivet', ?2, ?2)",
        (conversation, stamp(started)),
    )?;
    for index in 0..MESSAGES {
        let role = if index % 2 == 0 { "user" } else { "assistant" };
        conn.execute(
            "INSERT INTO ros_messages (id, conversation_id, agent, channel, role, content, created_at)
             VALUES (?1, ?2, 'rivet', 'test', ?3, ?4, ?5)",
            (
                format!("0199e2a0{separator}7b3c{separator}7a00{separator}8001{separator}{index:012}"),
                conversation,
                role,
                format!("ledger step {index}"),
                stamp(started + chrono::TimeDelta::seconds(i64::from(index))),
            ),
        )?;
    }
    Ok(())
}

fn summary(path: &Path, conversation: &str) -> TestResult<Option<String>> {
    let conn = rusqlite::Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    Ok(conn
        .query_row(
            "SELECT s.content FROM ros_summaries s
               JOIN ros_summary_sources src ON src.summary_id = s.id
               JOIN ros_messages m ON m.id = src.message_id
              WHERE m.conversation_id = ?1
              LIMIT 1",
            [conversation],
            |row| row.get(0),
        )
        .optional()?)
}

async fn wait_summary(path: &Path, conversation: &str) -> TestResult<Option<String>> {
    let waited = tokio::time::timeout(IDLE_TICK + Duration::from_secs(60), async {
        loop {
            match summary(path, conversation) {
                Ok(Some(content)) => return Ok(content),
                Ok(None) => tokio::task::yield_now().await,
                Err(error) => return Err(error.to_string()),
            }
        }
    })
    .await;
    match waited {
        Ok(inner) => inner.map(Some).map_err(Into::into),
        Err(_) => Ok(None),
    }
}

async fn summarized_within_one_idle_tick(
    home: &Path,
    database: &Path,
    conversation: &str,
) -> TestResult {
    let stub = model_stub().await?;
    let port = free_port()?;
    write_config(
        home,
        &provider_base(
            home,
            &format!("http://{}", stub.address),
            &format!(
                "memory:\n  backend: sqlite\n  sqlite:\n    path: {}\n    workers: true\nden:\n  host: 127.0.0.1\n  port: {port}\n",
                database.display()
            ),
        ),
    )?;
    let started = start(
        home,
        &["local", "--no-lan"],
        &["gateway listening on ", "worker running summarizer"],
    )
    .await?;
    let summarized = wait_summary(database, conversation).await;
    let logs = stop(started).await;
    let closed = stub.close().await;
    let logs = logs?;
    closed?;
    assert_eq!(summarized?.as_deref(), Some(DELEGATE_TEXT), "{logs}");
    Ok(())
}

#[tokio::test]
async fn an_idle_sqlite_conversation_is_summarized_within_one_idle_tick() -> TestResult {
    let home = tempfile::tempdir()?;
    let database = home.path().join(".rivethub").join("memory.sqlite");
    seed_idle_conversation(&database, CONVERSATION, "-")?;
    summarized_within_one_idle_tick(home.path(), &database, CONVERSATION).await
}

#[tokio::test]
async fn an_imported_undashed_conversation_is_summarized_after_migrating() -> TestResult {
    let home = tempfile::tempdir()?;
    let database = home.path().join(".rivethub").join("memory.sqlite");
    seed_idle_conversation(&database, IMPORTED, "")?;
    rusqlite::Connection::open(&database)?.execute_batch("PRAGMA user_version = 9;")?;
    migrate::sqlite(&database)?;
    summarized_within_one_idle_tick(home.path(), &database, IMPORTED_CANONICAL).await
}
