mod common;

use kernel::queue::{JobQueue, WorkerRole};
use kernel::tasks::{ClaimSelector, TaskJobs};
use memory_postgres::{PostgresQueue, PostgresTasks};
use sqlx::PgPool;
use std::time::Duration;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn add(pool: &PgPool, queue: Option<&str>) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT (graphile_worker.add_job('embed-target', '{}'::json, $1)).id")
        .bind(queue)
        .fetch_one(pool)
        .await
}

async fn owner(pool: &PgPool, queue: &str) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT locked_by FROM graphile_worker._private_job_queues WHERE queue_name = $1",
    )
    .bind(queue)
    .fetch_one(pool)
    .await
}

#[sqlx::test(migrations = false)]
async fn concurrent_workers_serialize_each_named_queue_and_release_on_completion(
    pool: PgPool,
) -> TestResult {
    common::install_graphile(&pool).await?;
    let first = add(&pool, Some("serial")).await?;
    let second = add(&pool, Some("serial")).await?;
    let left = PostgresQueue::connect(pool.clone()).await?;
    let right = PostgresQueue::connect(pool.clone()).await?;
    let (a, b) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(
            left.claim(WorkerRole::Embedder),
            right.claim(WorkerRole::Embedder)
        )
    })
    .await?;
    let (winner, loser, job) = match (a?, b?) {
        (Some(job), None) => (&left, &right, job),
        (None, Some(job)) => (&right, &left, job),
        other => return Err(format!("one queue has exactly one owner: {other:?}").into()),
    };
    assert_eq!(i64::from(job.id), first);
    let locked = owner(&pool, "serial").await?.ok_or("queue lock missing")?;
    let job_owner: String =
        sqlx::query_scalar("SELECT locked_by FROM graphile_worker._private_jobs WHERE id = $1")
            .bind(first)
            .fetch_one(&pool)
            .await?;
    assert_eq!(locked, job_owner);
    loser.complete(job.id).await?;
    loser.fail(job.id, "foreign worker").await?;
    assert_eq!(owner(&pool, "serial").await?, Some(locked));
    assert!(loser.claim(WorkerRole::Embedder).await?.is_none());
    winner.complete(job.id).await?;
    assert_eq!(owner(&pool, "serial").await?, None);
    let next = loser
        .claim(WorkerRole::Embedder)
        .await?
        .ok_or("second job")?;
    assert_eq!(i64::from(next.id), second);
    loser.complete(next.id).await?;
    assert_eq!(owner(&pool, "serial").await?, None);
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn failure_unlocks_the_queue_and_preserves_retry_metadata(pool: PgPool) -> TestResult {
    common::install_graphile(&pool).await?;
    let first = add(&pool, Some("serial")).await?;
    let second = add(&pool, Some("serial")).await?;
    let queue = PostgresQueue::connect(pool.clone()).await?;
    let job = queue
        .claim(WorkerRole::Embedder)
        .await?
        .ok_or("first job")?;
    queue.fail(job.id, "retry me").await?;
    assert_eq!(owner(&pool, "serial").await?, None);
    let state: (i16, Option<String>, Option<String>, bool) = sqlx::query_as(
        "SELECT attempts, last_error, locked_by, run_at > now() FROM graphile_worker._private_jobs WHERE id = $1",
    ).bind(first).fetch_one(&pool).await?;
    assert_eq!(state, (1, Some("retry me".to_owned()), None, true));
    let next = queue
        .claim(WorkerRole::Embedder)
        .await?
        .ok_or("second job")?;
    assert_eq!(i64::from(next.id), second);
    queue.complete(next.id).await?;
    sqlx::query("UPDATE graphile_worker._private_jobs SET run_at = now() WHERE id = $1")
        .bind(first)
        .execute(&pool)
        .await?;
    let retry = queue.claim(WorkerRole::Embedder).await?.ok_or("retry")?;
    assert_eq!(retry.attempt, 2);
    queue.complete(retry.id).await?;
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn locked_queues_do_not_block_other_queues_or_unnamed_jobs(pool: PgPool) -> TestResult {
    common::install_graphile(&pool).await?;
    add(&pool, Some("busy")).await?;
    let independent = add(&pool, Some("independent")).await?;
    let unnamed = add(&pool, None).await?;
    let mut lock = pool.begin().await?;
    sqlx::query(
        "SELECT id FROM graphile_worker._private_job_queues WHERE queue_name = 'busy' FOR UPDATE",
    )
    .execute(&mut *lock)
    .await?;
    let queue = PostgresQueue::connect(pool.clone()).await?;
    let first = tokio::time::timeout(Duration::from_secs(10), queue.claim(WorkerRole::Embedder))
        .await??
        .ok_or("independent queue")?;
    assert_eq!(i64::from(first.id), independent);
    let second = queue
        .claim(WorkerRole::Embedder)
        .await?
        .ok_or("unnamed job")?;
    assert_eq!(i64::from(second.id), unnamed);
    assert!(queue.claim(WorkerRole::Embedder).await?.is_none());
    lock.rollback().await?;
    assert!(queue.claim(WorkerRole::Embedder).await?.is_some());
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn expired_locks_are_reclaimed_and_old_workers_cannot_release_the_new_owner(
    pool: PgPool,
) -> TestResult {
    common::install_graphile(&pool).await?;
    add(&pool, Some("serial")).await?;
    let old = PostgresQueue::connect(pool.clone()).await?;
    let new = PostgresQueue::connect(pool.clone()).await?;
    let claimed = old.claim(WorkerRole::Embedder).await?.ok_or("old claim")?;
    sqlx::query("UPDATE graphile_worker._private_jobs SET locked_at = now() - interval '5 hours'")
        .execute(&pool)
        .await?;
    assert!(new.claim(WorkerRole::Embedder).await?.is_none());
    sqlx::query(
        "UPDATE graphile_worker._private_job_queues SET locked_at = now() - interval '5 hours'",
    )
    .execute(&pool)
    .await?;
    let reclaimed = new
        .claim(WorkerRole::Embedder)
        .await?
        .ok_or("expired claim")?;
    assert_eq!(claimed.id, reclaimed.id);
    let locked = owner(&pool, "serial").await?;
    old.fail(claimed.id, "stale failure").await?;
    old.complete(claimed.id).await?;
    assert_eq!(owner(&pool, "serial").await?, locked);
    new.complete(reclaimed.id).await?;
    assert_eq!(owner(&pool, "serial").await?, None);
    Ok(())
}

#[sqlx::test(migrations = false)]
async fn task_and_worker_claims_share_named_queue_exclusion(pool: PgPool) -> TestResult {
    common::install_graphile(&pool).await?;
    let first = add(&pool, Some("serial")).await?;
    let second = add(&pool, Some("serial")).await?;
    let tasks = PostgresTasks::from_pool(pool.clone());
    let other = PostgresTasks::from_pool(pool.clone());
    let queue = PostgresQueue::connect(pool.clone()).await?;
    let selector = ClaimSelector {
        names: vec!["embed-target".to_owned()],
        only_id: Some(first),
    };
    let job = tasks.claim(&selector).await?.ok_or("task claim")?;
    assert!(other.complete(job.id).await.is_err());
    assert!(other.fail(job.id, "foreign").await.is_err());
    assert!(queue.claim(WorkerRole::Embedder).await?.is_none());
    tasks.fail(job.id, "retry").await?;
    assert_eq!(owner(&pool, "serial").await?, None);
    let selector = ClaimSelector {
        only_id: Some(second),
        ..selector
    };
    let job = tasks.claim(&selector).await?.ok_or("second task claim")?;
    tasks.complete(job.id).await?;
    assert_eq!(owner(&pool, "serial").await?, None);
    Ok(())
}
