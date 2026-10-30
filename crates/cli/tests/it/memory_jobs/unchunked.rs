use config::{Config, Environment};
use kernel::memory::jobs::{ChunkWrite, MemoryJobStore};
use kernel::memory::{EmbedTable, EmbedTarget};
use kernel::queue::{FixedJob, Job, JobKeyMode, JobName};
use memory_postgres::PostgresMemory;
use serde::Serialize;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::JsonObject;
use workers::EmbedSettings;
use workers::embedding_sweep::handle_enqueue_unchunked;

use super::support::{RecordingQueue, TestResult, seed};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetPayload {
    target_table: &'static str,
    target_id: String,
}

const CONVERSATION: &str = "00000000-0000-4000-8000-000000000001";

fn window(chars: u32) -> EmbedSettings {
    EmbedSettings {
        chars_per_chunk: chars,
        ..EmbedSettings::standard()
    }
}

async fn message(
    pool: &PgPool,
    id: u16,
    content: &str,
    tool_result: Option<&str>,
    status: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    let offset = f64::from(id);
    let id = Uuid::from_u128(u128::from(id));
    sqlx::query(
        "INSERT INTO rh_messages (id, conversation_id, agent, channel, role, content, tool_result, embed_status, created_at, owner_user_id)
         VALUES ($1, $2::uuid, 'test', 'test', 'user', $3, $4, $5, '2026-10-01T00:00:00Z'::timestamptz + make_interval(secs => $6), 'owner')",
    )
    .bind(id)
    .bind(CONVERSATION)
    .bind(content)
    .bind(tool_result)
    .bind(status)
    .bind(offset)
    .execute(pool)
    .await?;
    Ok(id)
}

fn chunk_job(id: &str, max_attempts: i32) -> Result<Job, serde_json::Error> {
    Ok(Job {
        name: JobName::Fixed(FixedJob::EmbedTarget),
        payload: JsonObject::from_serialize(&TargetPayload {
            target_table: "ros_message_chunks",
            target_id: id.to_owned(),
        })?,
        key: Some(format!("embed-ros_message_chunks-{id}")),
        key_mode: JobKeyMode::KeepRunAt,
        priority: 0,
        run_at: None,
        max_attempts: Some(max_attempts),
    })
}

async fn chunk_rows(
    pool: &PgPool,
    message: Uuid,
) -> Result<Vec<(String, i32, i32, i32, bool)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id::text, idx, char_start, char_end, embedding IS NULL FROM rh_message_chunks
         WHERE message_id = $1 ORDER BY idx",
    )
    .bind(message)
    .fetch_all(pool)
    .await
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn unchunked_selection_follows_the_length_and_status_predicate(pool: PgPool) -> TestResult {
    let store = seed(&pool).await;
    RecordingQueue::new(&pool).await;
    let long = "a".repeat(21);
    let exact = "a".repeat(20);
    let over = message(&pool, 101, &long, None, None).await?;
    message(&pool, 102, &exact, None, None).await?;
    message(&pool, 103, &long, None, Some("unembeddable")).await?;
    let failed = message(&pool, 104, &long, None, Some("failed")).await?;
    let tool_only = message(&pool, 105, "", Some(&long), None).await?;
    message(&pool, 106, &"b".repeat(10), Some(&"c".repeat(9)), None).await?;
    let joined = message(&pool, 107, &"b".repeat(10), Some(&"c".repeat(10)), None).await?;
    message(&pool, 108, &format!(" \t{exact}\n\x0b\x0c\r"), None, None).await?;
    let padded = message(&pool, 109, &format!("  {long}  "), None, None).await?;
    let chunked = message(&pool, 110, &long, None, None).await?;
    let pending = message(&pool, 111, &long, None, Some("pending")).await?;
    let capped = message(&pool, 112, "", Some(&"d".repeat(30)), None).await?;
    let lettered = message(&pool, 113, &format!("v{exact}"), None, None).await?;
    sqlx::query("INSERT INTO rh_message_chunks (message_id, idx, char_start, char_end, content, owner_user_id) VALUES ($1, 0, 0, 21, 'x', 'owner')")
        .bind(chunked)
        .execute(&pool)
        .await?;
    let ids = |rows: Vec<kernel::memory::jobs::UnchunkedMessage>| -> Vec<Uuid> {
        rows.into_iter().map(|row| row.id).collect()
    };
    assert_eq!(
        ids(store.unchunked(100, 20, 32_768).await?),
        vec![
            lettered, capped, pending, padded, joined, tool_only, failed, over
        ]
    );
    assert_eq!(
        ids(store.unchunked(100, 20, 15).await?),
        vec![lettered, pending, padded, joined, failed, over]
    );
    assert_eq!(
        ids(store.unchunked(2, 20, 32_768).await?),
        vec![lettered, capped]
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn unchunked_writes_bare_chunks_and_one_embed_target_per_null_chunk(
    pool: PgPool,
) -> TestResult {
    let store = seed(&pool).await;
    let queue = RecordingQueue::new(&pool).await;
    let cancel = CancellationToken::new();
    let id = message(&pool, 201, &"界".repeat(45), None, None).await?;
    assert_eq!(
        handle_enqueue_unchunked(
            &store,
            &queue,
            window(20).with_chunks_enabled(false),
            200,
            7,
            &cancel
        )
        .await?,
        0
    );
    assert_eq!(
        handle_enqueue_unchunked(&store, &queue, window(20), 200, 7, &cancel).await?,
        3
    );
    let rows = chunk_rows(&pool, id).await?;
    let spans: Vec<_> = rows
        .iter()
        .map(|(_, idx, start, end, bare)| (*idx, *start, *end, *bare))
        .collect();
    assert_eq!(
        spans,
        vec![(0, 0, 20, true), (1, 20, 40, true), (2, 40, 45, true)]
    );
    let expected = rows
        .iter()
        .map(|(chunk, ..)| chunk_job(chunk, 7))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(*queue.jobs.lock(), expected);
    let stored: Vec<(String, i16, i16)> = sqlx::query_as(
        "SELECT key, max_attempts, priority FROM graphile_worker._private_jobs
         WHERE key LIKE 'embed-ros_message_chunks-%' ORDER BY id",
    )
    .fetch_all(&pool)
    .await?;
    assert_eq!(
        stored,
        rows.iter()
            .map(|(chunk, ..)| (format!("embed-ros_message_chunks-{chunk}"), 7, 0))
            .collect::<Vec<_>>()
    );
    queue.jobs.lock().clear();
    assert_eq!(
        handle_enqueue_unchunked(&store, &queue, window(20), 200, 7, &cancel).await?,
        0
    );
    assert!(queue.jobs.lock().is_empty());
    assert_eq!(
        store.write_chunks(id, Vec::new()).await?,
        ChunkWrite::Skipped
    );
    assert_eq!(
        store.write_chunks(Uuid::from_u128(999), Vec::new()).await?,
        ChunkWrite::Skipped
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn unchunked_skips_a_failing_message_and_survives_a_failing_sweep(
    pool: PgPool,
) -> TestResult {
    let store = seed(&pool).await;
    let queue = RecordingQueue::new(&pool).await;
    let cancel = CancellationToken::new();
    let refused = message(&pool, 301, &"x".repeat(30), None, None).await?;
    let accepted = message(&pool, 302, &"y".repeat(30), None, None).await?;
    sqlx::query(
        "ALTER TABLE rh_message_chunks ADD CONSTRAINT refuse_x CHECK (content NOT LIKE 'x%')",
    )
    .execute(&pool)
    .await?;
    assert_eq!(
        handle_enqueue_unchunked(&store, &queue, window(20), 200, 5, &cancel).await?,
        2
    );
    assert_eq!(chunk_rows(&pool, refused).await?, Vec::new());
    assert_eq!(chunk_rows(&pool, accepted).await?.len(), 2);
    sqlx::query("ALTER TABLE rh_message_chunks RENAME TO parked_chunks")
        .execute(&pool)
        .await?;
    assert_eq!(
        handle_enqueue_unchunked(&store, &queue, window(20), 200, 5, &cancel).await?,
        0
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn configured_chunk_window_drives_embed_target_and_the_sweep(pool: PgPool) -> TestResult {
    let env = Environment::default();
    let document = |chars: u32| {
        format!(
            "runtime:\n  workspace: /home/user/work\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    model: m\nmemory:\n  postgres:\n    chars_per_chunk: {chars}\n"
        )
    };
    let loaded = Config::from_str(&env, &document(4000))?;
    let settings = EmbedSettings::from_config(&loaded.config)?;
    assert_eq!(settings.chars_per_chunk, 4000);
    assert_eq!(EmbedSettings::standard().chars_per_chunk, 6000);
    assert!(Config::from_str(&env, &document(0)).is_err());
    let store: PostgresMemory = seed(&pool).await;
    let queue = RecordingQueue::new(&pool).await;
    let cancel = CancellationToken::new();
    let prose = "word ".repeat(799);
    let embedded = message(&pool, 401, &format!("{prose}ending"), None, None).await?;
    let swept = message(&pool, 402, &format!("{prose}ending"), None, None).await?;
    let exact = message(&pool, 403, &format!("{prose}words"), None, None).await?;
    let provider = testkit::FakeProvider::fnv_embeddings(1024);
    for id in [embedded, exact] {
        let payload = JsonObject::from_serialize(&TargetPayload {
            target_table: "ros_messages",
            target_id: id.to_string(),
        })?;
        workers::handle_embed(&store, &provider, settings, &payload, &cancel).await?;
    }
    let ends = |rows: Vec<(String, i32, i32, i32, bool)>| -> Vec<(i32, bool)> {
        rows.into_iter().map(|row| (row.3, row.4)).collect()
    };
    assert_eq!(
        ends(chunk_rows(&pool, embedded).await?),
        vec![(4000, false), (4001, false)]
    );
    assert_eq!(chunk_rows(&pool, exact).await?, Vec::new());
    assert_eq!(
        handle_enqueue_unchunked(&store, &queue, settings, 200, 5, &cancel).await?,
        2
    );
    assert_eq!(
        ends(chunk_rows(&pool, swept).await?),
        vec![(4000, true), (4001, true)]
    );
    assert_eq!(chunk_rows(&pool, exact).await?, Vec::new());
    let targets: Vec<EmbedTarget> = store.unembedded_chunks(swept).await?;
    assert_eq!(targets.len(), 2);
    assert!(
        targets
            .iter()
            .all(|target| target.table == EmbedTable::MessageChunks)
    );
    Ok(())
}
