use kernel::memory::jobs::UnembeddedStore as _;
use kernel::memory::{EmbedTable, EmbedTarget};
use kernel::queue::{ClaimSelector, FixedJob, Job, JobKeyMode, JobName, JobQueue as _, WorkerRole};
use memory_sqlite::SqliteStore;
use serde::Serialize;
use wire::JsonObject;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const CONVERSATION: &str = "0199e2a0-7b3c-7a00-8000-0000000000c0";
const LONG: &str = "a message long enough to be worth embedding";

struct Opened {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    store: SqliteStore,
}

fn open() -> TestResult<Opened> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("memory.sqlite");
    migrate::sqlite(&path)?;
    let store = SqliteStore::open(&path)?;
    let conn = rusqlite::Connection::open(&path)?;
    conn.execute(
        "INSERT INTO ros_conversations (id, session_key, agent, created_at, updated_at)
         VALUES (?1, 'sweep', 'rivet', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
        [CONVERSATION],
    )?;
    Ok(Opened {
        _dir: dir,
        path,
        store,
    })
}

struct Row<'a> {
    id: &'a str,
    content: &'a str,
    tool_result: Option<&'a str>,
    status: Option<&'a str>,
    error: Option<&'a str>,
    minute: u32,
}

fn message(path: &std::path::Path, row: &Row<'_>) -> TestResult {
    rusqlite::Connection::open(path)?.execute(
        "INSERT INTO ros_messages (id, conversation_id, agent, channel, role, content, tool_result,
                                   created_at, embed_status, embed_error, embed_failures)
         VALUES (?1, ?2, 'rivet', 'test', 'user', ?3, ?4, ?5, ?6, ?7, 3)",
        rusqlite::params![
            row.id,
            CONVERSATION,
            row.content,
            row.tool_result,
            format!("2026-01-01T00:{:02}:00.000Z", row.minute),
            row.status,
            row.error
        ],
    )?;
    Ok(())
}

fn id(n: u32) -> String {
    format!("0199e2a0-7b3c-7a00-8000-0000000001{n:02}")
}

fn targets(table: EmbedTable, ids: &[String]) -> Vec<EmbedTarget> {
    ids.iter()
        .map(|id| EmbedTarget {
            table,
            id: id.clone(),
        })
        .collect()
}

fn seed_messages(path: &std::path::Path) -> TestResult {
    let rows = [
        (1, LONG, None, None, None),
        (2, "short", None, None, None),
        (3, "short", Some(LONG), None, None),
        (4, LONG, None, Some("done"), None),
        (
            5,
            LONG,
            None,
            Some("failed"),
            Some("Embedding returned null"),
        ),
        (
            6,
            LONG,
            None,
            Some("failed"),
            Some("Embedding returned null (permanent)"),
        ),
        (7, LONG, None, Some("unembeddable"), Some("too short")),
    ];
    for (n, content, tool_result, status, error) in rows {
        message(
            path,
            &Row {
                id: &id(n),
                content,
                tool_result,
                status,
                error,
                minute: n,
            },
        )?;
    }
    Ok(())
}

#[tokio::test]
async fn unembedded_messages_are_selected_newest_first_and_null_poison_is_healed() -> TestResult {
    let opened = open()?;
    seed_messages(&opened.path)?;
    let found = opened.store.unembedded(EmbedTable::Messages, 10).await?;
    assert_eq!(found, targets(EmbedTable::Messages, &[id(5), id(3), id(1)]));
    let healed: (Option<String>, Option<String>, i64) = rusqlite::Connection::open(&opened.path)?
        .query_row(
        "SELECT embed_status, embed_error, embed_failures FROM ros_messages WHERE id = ?1",
        [id(5)],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(healed, (None, None, 0));
    let limited = opened.store.unembedded(EmbedTable::Messages, 1).await?;
    assert_eq!(limited, targets(EmbedTable::Messages, &[id(5)]));
    Ok(())
}

#[tokio::test]
async fn summaries_and_wiki_topics_are_swept_and_chunks_have_no_rows() -> TestResult {
    let opened = open()?;
    let conn = rusqlite::Connection::open(&opened.path)?;
    conn.execute(
        "INSERT INTO ros_summaries (id, conversation_id, content, created_at) VALUES
           (?1, ?3, ?4, '2026-01-01T00:01:00.000Z'),
           (?2, ?3, 'tiny', '2026-01-01T00:02:00.000Z')",
        rusqlite::params![id(10), id(11), CONVERSATION, LONG],
    )?;
    conn.execute(
        "INSERT INTO ros_wiki_topics (slug, title, search_text, created_at, updated_at) VALUES
           ('deploys', 'Deploys', ?1, '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z'),
           ('stub', 'Stub', 'tiny', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
        [LONG],
    )?;
    assert_eq!(
        opened.store.unembedded(EmbedTable::Summaries, 10).await?,
        targets(EmbedTable::Summaries, &[id(10)])
    );
    assert_eq!(
        opened.store.unembedded(EmbedTable::WikiTopics, 10).await?,
        targets(EmbedTable::WikiTopics, &["deploys".to_owned()])
    );
    assert_eq!(
        opened
            .store
            .unembedded(EmbedTable::MessageChunks, 10)
            .await?,
        Vec::new()
    );
    Ok(())
}

#[derive(Serialize)]
struct Target<'a> {
    #[serde(rename = "targetTable")]
    target_table: &'a str,
    #[serde(rename = "targetId")]
    target_id: &'a str,
}

#[tokio::test]
async fn the_embedder_claims_the_sweep_and_a_sweep_revives_a_dead_embed_job() -> TestResult {
    let opened = open()?;
    let store = &opened.store;
    assert!(
        store
            .task_names(WorkerRole::Embedder)
            .contains(&JobName::Fixed(FixedJob::EnqueueUnembedded))
    );
    let key = format!("embed-ros_messages-{}", id(1));
    rusqlite::Connection::open(&opened.path)?.execute(
        "INSERT INTO ros_jobs (id, task, job_key, payload, run_at, attempts, max_attempts, state,
                               last_error, created_at, updated_at)
         VALUES ('0199e2a0-7b3c-7a00-8000-0000000002ff', 'embed-target', ?1, '{}',
                 '2026-01-01T00:00:00.000Z', 5, 5, 'dead', 'endpoint down',
                 '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
        [&key],
    )?;
    store
        .enqueue(Job {
            name: JobName::Fixed(FixedJob::EmbedTarget),
            payload: JsonObject::from_serialize(&Target {
                target_table: "ros_messages",
                target_id: &id(1),
            })?,
            key: Some(key),
            key_mode: JobKeyMode::KeepRunAt,
            priority: 0,
            run_at: None,
            max_attempts: Some(5),
        })
        .await?;
    let claimed = store
        .claim(&ClaimSelector::matching(
            store.task_names(WorkerRole::Embedder),
        ))
        .await?
        .ok_or("the revived embed job is claimable")?;
    assert_eq!(claimed.name, JobName::Fixed(FixedJob::EmbedTarget));
    assert_eq!(claimed.attempt, 1);
    assert_eq!(
        claimed
            .payload
            .get("targetId")
            .and_then(|value| value.as_str()),
        Some(id(1).as_str())
    );
    Ok(())
}
