use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue as _;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use memory_postgres::PostgresMemory;
use memory_postgres::PostgresQueue;
use sqlx::PgPool;
use wire::JsonObject;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn object<T: serde::Serialize>(payload: &T) -> JsonObject {
    serde_json::from_value(serde_json::to_value(payload).unwrap_or_default()).unwrap_or_default()
}

#[derive(serde::Serialize)]
struct EmbedTargetPayload<'a> {
    #[serde(rename = "targetTable")]
    target_table: &'a str,
    #[serde(rename = "targetId")]
    target_id: &'a str,
}

#[derive(serde::Serialize)]
struct CompactConversationPayload<'a> {
    #[serde(rename = "conversationId")]
    conversation_id: &'a str,
}

#[derive(serde::Serialize)]
struct ExtractWikiPayload<'a> {
    #[serde(rename = "summaryId")]
    summary_id: &'a str,
}

async fn queue(pool: &PgPool) -> Result<PostgresQueue, Box<dyn std::error::Error>> {
    Ok(PostgresQueue::connect(pool.clone()).await?)
}

fn store(pool: &PgPool) -> PostgresMemory {
    PostgresMemory::new(pool.clone())
}

async fn gated(pool: &PgPool) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(testkit::queue_present(pool).await?)
}

async fn stored_jobs(
    pool: &PgPool,
) -> Result<Vec<(String, Option<String>, i64, i64, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT t.identifier, j.key, j.priority::bigint AS priority,
                j.max_attempts::bigint AS max_attempts, j.attempts::bigint AS attempts
           FROM graphile_worker._private_jobs j
           JOIN graphile_worker._private_tasks t ON t.id = j.task_id
          ORDER BY j.id",
    )
    .fetch_all(pool)
    .await
}

fn embed_job() -> Job {
    Job {
        name: JobName::Fixed(FixedJob::EmbedTarget),
        payload: object(&EmbedTargetPayload {
            target_table: "ros_messages",
            target_id: "abc",
        }),
        key: Some("embed-ros_messages-abc".to_string()),
        key_mode: JobKeyMode::Replace,
        priority: 0,
        run_at: None,
    }
}

#[sqlx::test]
async fn enqueue_claims_and_completes_through_the_public_api(pool: PgPool) -> TestResult {
    if !gated(&pool).await? {
        return Ok(());
    }
    let jobs = queue(&pool).await?;

    jobs.enqueue(embed_job()).await?;
    let rows = stored_jobs(&pool).await?;
    assert_eq!(
        rows.first(),
        Some(&(
            "embed-target".to_string(),
            Some("embed-ros_messages-abc".to_string()),
            0,
            5,
            0
        ))
    );

    let claimed = jobs.claim(WorkerRole::Embedder).await?;
    let claimed = claimed.ok_or("the only job is claimable")?;
    assert_eq!(claimed.name, JobName::Fixed(FixedJob::EmbedTarget));
    assert_eq!(claimed.attempt, 1);
    assert!(
        claimed
            .payload
            .get("targetId")
            .and_then(|value| value.as_str())
            .is_some_and(|target| target == "abc")
    );
    assert!(
        jobs.claim(WorkerRole::Embedder).await?.is_none(),
        "the claimed job is locked"
    );

    jobs.fail(claimed.id, "boom").await?;
    assert!(
        jobs.claim(WorkerRole::Embedder).await?.is_none(),
        "a failed job waits out graphile's exponential backoff"
    );
    sqlx::query("UPDATE graphile_worker._private_jobs SET run_at = now()")
        .execute(&pool)
        .await?;
    let retried = jobs
        .claim(WorkerRole::Embedder)
        .await?
        .ok_or("the failed job is retried after its backoff")?;
    assert_eq!(retried.id, claimed.id);
    assert_eq!(retried.attempt, 2);

    jobs.complete(retried.id).await?;
    assert!(
        jobs.claim(WorkerRole::Embedder).await?.is_none(),
        "completion removes the job"
    );
    Ok(())
}

#[sqlx::test]
async fn job_keys_replace_preserve_and_revive(pool: PgPool) -> TestResult {
    if !gated(&pool).await? {
        return Ok(());
    }
    let jobs = queue(&pool).await?;
    let compact = |payload: JsonObject| Job {
        name: JobName::Fixed(FixedJob::CompactConversation),
        payload,
        key: Some("conv-key".to_string()),
        key_mode: JobKeyMode::Replace,
        priority: 0,
        run_at: None,
    };

    jobs.enqueue(compact(object(&CompactConversationPayload {
        conversation_id: "one",
    })))
    .await?;
    jobs.enqueue(compact(object(&CompactConversationPayload {
        conversation_id: "two",
    })))
    .await?;
    let rows = stored_jobs(&pool).await?;
    assert_eq!(rows.len(), 1, "replace must reuse the keyed row");

    sqlx::query("UPDATE graphile_worker._private_jobs SET attempts = max_attempts")
        .execute(&pool)
        .await?;
    jobs.enqueue(compact(object(&CompactConversationPayload {
        conversation_id: "three",
    })))
    .await?;
    let rows = stored_jobs(&pool).await?;
    assert_eq!(
        rows.len(),
        2,
        "a dead keyed row becomes a corpse and a fresh row"
    );
    let corpse = rows.first().cloned().ok_or("the corpse row")?;
    let fresh = rows.get(1).cloned().ok_or("the fresh row")?;
    assert_eq!(corpse.1, None, "the corpse loses its key");
    assert_eq!(fresh.1.as_deref(), Some("conv-key"));
    assert_eq!(fresh.4, 0, "the fresh row restarts attempts");
    Ok(())
}

#[sqlx::test]
async fn role_scoping_keeps_the_task_lists_apart(pool: PgPool) -> TestResult {
    if !gated(&pool).await? {
        return Ok(());
    }
    let jobs = queue(&pool).await?;
    assert!(
        jobs.claim(WorkerRole::Summarizer).await?.is_none(),
        "an empty queue claims nothing"
    );

    jobs.enqueue(embed_job()).await?;
    assert!(
        jobs.claim(WorkerRole::Summarizer).await?.is_none(),
        "the summarizer role must not claim embed jobs"
    );
    let embedder_claim = jobs.claim(WorkerRole::Embedder).await?;
    let claimed = embedder_claim.ok_or("the embedder claims its own job")?;
    jobs.complete(claimed.id).await?;

    jobs.enqueue(Job {
        name: JobName::Fixed(FixedJob::ExtractWiki),
        payload: object(&ExtractWikiPayload { summary_id: "s1" }),
        key: Some("wiki-ext-s1".to_string()),
        key_mode: JobKeyMode::Replace,
        priority: 5,
        run_at: None,
    })
    .await?;
    sqlx::query(
        "UPDATE graphile_worker._private_jobs
            SET attempts = max_attempts, updated_at = now() - interval '2 days'",
    )
    .execute(&pool)
    .await?;

    assert_eq!(
        jobs.reschedule_dead(&JobName::Fixed(FixedJob::ExtractWiki), 200, 5)
            .await?,
        1
    );
    let revived = stored_jobs(&pool).await?;
    assert_eq!(
        revived.first().map(|row| row.4),
        Some(0),
        "revive resets attempts"
    );

    sqlx::query(
        "UPDATE graphile_worker._private_jobs
            SET attempts = max_attempts, key = null,
                updated_at = now() - interval '8 days'",
    )
    .execute(&pool)
    .await?;
    let reaped = jobs
        .reap_dead_corpses(
            &[
                JobName::Fixed(FixedJob::ExtractWiki),
                JobName::Fixed(FixedJob::CompactConversation),
                JobName::Fixed(FixedJob::EmbedTarget),
            ],
            200,
        )
        .await?;
    assert_eq!(reaped, 1, "the keyless corpse is reaped");
    assert_eq!(stored_jobs(&pool).await?, Vec::new());
    Ok(())
}

#[sqlx::test]
async fn queue_health_reports_buckets(pool: PgPool) -> TestResult {
    if !gated(&pool).await? {
        return Ok(());
    }
    let jobs = queue(&pool).await?;
    jobs.enqueue(Job {
        name: JobName::Fixed(FixedJob::ExtractWiki),
        payload: object(&ExtractWikiPayload { summary_id: "s1" }),
        key: None,
        key_mode: JobKeyMode::Replace,
        priority: 5,
        run_at: None,
    })
    .await?;
    sqlx::query("UPDATE graphile_worker._private_jobs SET attempts = max_attempts")
        .execute(&pool)
        .await?;

    let health = jobs
        .queue_health(&JobName::Fixed(FixedJob::ExtractWiki))
        .await?;
    let health = health.ok_or("the health row exists")?;
    assert_eq!(health.task, JobName::Fixed(FixedJob::ExtractWiki));
    assert_eq!(health.dead, 1);
    assert_eq!(health.pending, 0);

    assert!(
        jobs.queue_health(&JobName::Fixed(FixedJob::CompactConversation))
            .await?
            .is_none(),
        "a task without jobs reports no bucket"
    );
    Ok(())
}

#[sqlx::test]
async fn the_store_reports_the_queue_as_unavailable(pool: PgPool) -> TestResult {
    if gated(&pool).await? {
        return Ok(());
    }
    let jobs = queue(&pool).await?;
    let refused = jobs.enqueue(embed_job()).await;
    assert!(
        matches!(refused, Err(JobQueueError::Unavailable(_))),
        "enqueue without the graphile schema is refused: {refused:?}"
    );
    let refused = jobs.claim(WorkerRole::Embedder).await;
    assert!(matches!(refused, Err(JobQueueError::Unavailable(_))));
    let refused = jobs.complete(kernel::queue::JobId::new(1)).await;
    assert!(matches!(refused, Err(JobQueueError::Unavailable(_))));
    let refused = jobs.fail(kernel::queue::JobId::new(1), "boom").await;
    assert!(matches!(refused, Err(JobQueueError::Unavailable(_))));

    let memory = store(&pool);
    assert!(!memory.queue_available().await?);
    Ok(())
}

#[test]
fn crontab_and_task_lists_follow_the_roles() {
    let embedder = PostgresQueue::crontab(WorkerRole::Embedder);
    assert_eq!(embedder.len(), 2);
    assert_eq!(
        embedder.first().map(|entry| entry.schedule.as_str()),
        Some("*/10 * * * *")
    );
    assert_eq!(
        embedder.get(1).map(|entry| entry.identifier.as_str()),
        Some("unchunked-sweep")
    );
    let summarizer = PostgresQueue::crontab(WorkerRole::Summarizer);
    assert_eq!(summarizer.len(), 3);
    assert_eq!(PostgresQueue::crontab(WorkerRole::Categorizer).len(), 0);

    assert_eq!(
        PostgresQueue::task_list(WorkerRole::Embedder),
        &[JobName::Fixed(FixedJob::EmbedTarget)]
    );
    assert!(
        PostgresQueue::task_list(WorkerRole::Summarizer)
            .contains(&JobName::Fixed(FixedJob::CompactConversation))
    );

    assert_eq!(
        PostgresQueue::heartbeat_schedule("5"),
        Some("*/5 * * * *".to_string())
    );
    assert_eq!(
        PostgresQueue::heartbeat_schedule("2h"),
        Some("0 */2 * * *".to_string())
    );
    assert_eq!(
        PostgresQueue::heartbeat_schedule("*/15 * * * *"),
        Some("*/15 * * * *".to_string())
    );
    assert_eq!(PostgresQueue::heartbeat_schedule("weekly"), None);
}

#[test]
fn job_names_map_to_recorded_max_attempts() {
    let attempts = |name: JobName| {
        let payload = JsonObject::default();
        Job {
            name,
            payload,
            key: None,
            key_mode: JobKeyMode::Replace,
            priority: 0,
            run_at: None,
        }
        .name
        .to_string()
    };
    assert_eq!(
        attempts(JobName::Fixed(FixedJob::EmbedTarget)),
        "embed-target"
    );
    assert_eq!(
        attempts(JobName::Fixed(FixedJob::CompactConversation)),
        "compact-conversation"
    );
    assert_eq!(
        attempts(JobName::RunTaskOn {
            node: "east".to_string()
        }),
        "run-task:east"
    );
    assert_eq!(
        attempts(JobName::Heartbeat {
            agent: "rivet".to_string(),
            index: 2
        }),
        "heartbeat-rivet-2"
    );
}
