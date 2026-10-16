use chrono::{DateTime, Utc};
use kernel::queue::{FixedJob, Job, JobKeyMode, JobName, JobQueue, WorkerRole};
use kernel::wiki::builder::ExtractWiki;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::JsonObject;
use workers::embedding_sweep::{handle_enqueue_stale_wiki, handle_enqueue_unchunked};

use super::support::{RecordingQueue, TestResult, message_id, seed};

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn unchunked_repairs_long_embedded_messages_and_preserves_run_at(pool: PgPool) -> TestResult {
    let store = seed(&pool).await;
    let queue = RecordingQueue::new(&pool).await;
    sqlx::query("UPDATE rh_messages SET content = repeat('界', 21), embedding = array_fill(1::real, ARRAY[1024])::halfvec").execute(&pool).await?;
    let settings = workers::EmbedSettings {
        chars_per_chunk: 20,
        ..workers::EmbedSettings::standard()
    };
    let target = kernel::memory::EmbedTarget {
        table: kernel::memory::EmbedTable::Messages,
        id: message_id().to_string(),
    };
    let expected = workers::embedding_sweep::embed_sweep_job(&target, 5)?;
    let future: DateTime<Utc> = "2099-01-01T00:00:00Z".parse()?;
    let mut scheduled = expected.clone();
    scheduled.run_at = Some(future);
    queue.enqueue(scheduled).await?;
    queue.jobs.lock().clear();
    let cancel = CancellationToken::new();
    assert_eq!(
        handle_enqueue_unchunked(&store, &queue, settings, 100, 5, &cancel).await?,
        1
    );
    assert_eq!(*queue.jobs.lock(), vec![expected.clone()]);
    let actual: DateTime<Utc> =
        sqlx::query_scalar("SELECT run_at FROM graphile_worker._private_jobs WHERE key = $1")
            .bind(expected.key)
            .fetch_one(&pool)
            .await?;
    assert_eq!(actual, future);
    sqlx::query("INSERT INTO rh_message_chunks (message_id, idx, char_start, char_end, content) VALUES ($1, 0, 0, 21, repeat('界',21))").bind(message_id()).execute(&pool).await?;
    assert_eq!(
        handle_enqueue_unchunked(&store, &queue, settings, 100, 5, &cancel).await?,
        0
    );
    sqlx::query("DELETE FROM rh_message_chunks")
        .execute(&pool)
        .await?;
    sqlx::query("UPDATE rh_messages SET content = repeat('界',20)")
        .execute(&pool)
        .await?;
    assert_eq!(
        handle_enqueue_unchunked(&store, &queue, settings, 100, 5, &cancel).await?,
        0
    );
    sqlx::query("UPDATE rh_messages SET content = '', tool_result = repeat('界',21)")
        .execute(&pool)
        .await?;
    assert_eq!(
        handle_enqueue_unchunked(&store, &queue, settings, 100, 7, &cancel).await?,
        1
    );
    assert_eq!(
        handle_enqueue_unchunked(
            &store,
            &queue,
            settings.with_chunks_enabled(false),
            100,
            5,
            &cancel
        )
        .await?,
        0
    );
    sqlx::query("UPDATE rh_messages SET embedding = NULL")
        .execute(&pool)
        .await?;
    assert_eq!(
        handle_enqueue_unchunked(&store, &queue, settings, 100, 5, &cancel).await?,
        0
    );
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn stale_wiki_queues_only_outdated_successes_when_enabled(pool: PgPool) -> TestResult {
    let store = seed(&pool).await;
    let queue = RecordingQueue::new(&pool).await;
    for (id, status, version) in [
        (10, "done", 3),
        (11, "done", 4),
        (12, "failed", 3),
        (13, "skipped", 3),
    ] {
        sqlx::query("INSERT INTO rh_summaries (id, content) VALUES ($1, 'a summary')")
            .bind(Uuid::from_u128(id))
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO rh_wiki_extractions (summary_id,status,pipeline_version) VALUES ($1,$2,$3)").bind(Uuid::from_u128(id)).bind(status).bind(version).execute(&pool).await?;
    }
    let expected = Job {
        name: JobName::Fixed(FixedJob::ExtractWiki),
        payload: JsonObject::from_serialize(&ExtractWiki {
            summary_id: Uuid::from_u128(10),
            conversation_id: None,
        })?,
        key: Some(format!("wiki-ext-{}", Uuid::from_u128(10))),
        key_mode: JobKeyMode::KeepRunAt,
        priority: 10,
        run_at: None,
        max_attempts: Some(2),
    };
    let cancel = CancellationToken::new();
    assert_eq!(
        handle_enqueue_stale_wiki(&store, &queue, false, 100, &cancel).await?,
        0
    );
    assert!(queue.jobs.lock().is_empty());
    let future: DateTime<Utc> = "2099-01-01T00:00:00Z".parse()?;
    let mut scheduled = expected.clone();
    scheduled.run_at = Some(future);
    queue.enqueue(scheduled).await?;
    queue.jobs.lock().clear();
    assert_eq!(
        handle_enqueue_stale_wiki(&store, &queue, true, 100, &cancel).await?,
        1
    );
    assert_eq!(*queue.jobs.lock(), vec![expected.clone()]);
    let actual: DateTime<Utc> =
        sqlx::query_scalar("SELECT run_at FROM graphile_worker._private_jobs WHERE key = $1")
            .bind(expected.key)
            .fetch_one(&pool)
            .await?;
    assert_eq!(actual, future);
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn cron_registration_and_due_jobs_follow_roles_and_wiki_switch(pool: PgPool) -> TestResult {
    let queue = RecordingQueue::new(&pool).await;
    let cancel = CancellationToken::new();
    let entries: Vec<_> = workers::ProcessRole::All
        .selected()
        .into_iter()
        .flat_map(|role| role.crontab(true))
        .collect();
    assert_eq!(entries.len(), 7);
    assert_eq!(WorkerRole::WikiBuilder.crontab(false).len(), 1);
    assert!(entries.iter().all(|entry| entry.backfill_seconds == 0));
    workers::schedule::enqueue_due(&queue, &entries, "2026-10-16T12:30:00Z".parse()?, &cancel)
        .await?;
    let expected: Vec<_> = [
        FixedJob::EnqueueUnembedded,
        FixedJob::EnqueueUnchunked,
        FixedJob::EnqueueIdle,
        FixedJob::EnqueueStaleSummaries,
        FixedJob::EnqueueWikiBackfill,
        FixedJob::EnqueueStaleWiki,
    ]
    .into_iter()
    .map(|task| Job {
        name: JobName::Fixed(task),
        payload: JsonObject::default(),
        key: None,
        key_mode: JobKeyMode::Replace,
        priority: 0,
        run_at: None,
        max_attempts: Some(25),
    })
    .collect();
    assert_eq!(*queue.jobs.lock(), expected);
    Ok(())
}
