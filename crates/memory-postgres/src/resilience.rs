use std::time::Duration;

use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::tasks::TaskWatch;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::PostgresMemory;
use crate::PostgresTasks;

async fn terminate_other_backends(pool: &PgPool) {
    sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
         WHERE datname = current_database() AND pid <> pg_backend_pid() \
           AND backend_type = 'client backend'",
    )
    .execute(pool)
    .await
    .expect("terminate backends");
}

#[sqlx::test(migrations = false)]
async fn task_watch_resumes_after_its_backend_is_terminated(pool: PgPool) {
    let tasks = PostgresTasks::from_pool(pool.clone());
    let cancel = CancellationToken::new();
    let handle = tasks.spawn_watch(cancel.clone()).await.expect("listen");
    let mut watch = TaskWatch::subscribe(&tasks);
    terminate_other_backends(&pool).await;
    tokio::time::timeout(Duration::from_secs(20), watch.changed())
        .await
        .expect("reconnect kicks the watch")
        .expect("watch open");
    sqlx::query("SELECT pg_notify('ros_task_done', '1')")
        .execute(&pool)
        .await
        .expect("notify");
    tokio::time::timeout(Duration::from_secs(20), watch.changed())
        .await
        .expect("notification after reconnect")
        .expect("watch open");
    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(20), handle)
        .await
        .expect("watch joins")
        .expect("watch task");
    assert!(joined.is_ok());
}

#[sqlx::test(migrations = false)]
async fn task_watch_names_each_finished_task(pool: PgPool) {
    let tasks = PostgresTasks::from_pool(pool.clone());
    let cancel = CancellationToken::new();
    let handle = tasks.spawn_watch(cancel.clone()).await.expect("listen");
    let mut finished = TaskWatch::finished(&tasks);
    let id = uuid::Uuid::from_u128(42);
    for payload in ["1".to_owned(), id.to_string()] {
        sqlx::query("SELECT pg_notify('ros_task_done', $1)")
            .bind(payload)
            .execute(&pool)
            .await
            .expect("notify");
    }
    let named = tokio::time::timeout(Duration::from_secs(20), finished.recv())
        .await
        .expect("finished task named")
        .expect("finished open");
    assert_eq!(named, id);
    cancel.cancel();
    let joined = tokio::time::timeout(Duration::from_secs(20), handle)
        .await
        .expect("watch joins")
        .expect("watch task");
    assert!(joined.is_ok());
}

#[sqlx::test(migrations = false)]
async fn job_notifications_resume_after_the_backend_is_terminated(pool: PgPool) {
    sqlx::raw_sql(include_str!(
        "../../rivethub/tests/fixtures/graphile_worker.txt"
    ))
    .execute(&pool)
    .await
    .expect("graphile schema");
    let queue = PostgresMemory::new(pool.clone())
        .queue()
        .await
        .expect("queue");
    let mut notifications = queue.subscribe().await.expect("listen");
    terminate_other_backends(&pool).await;
    tokio::time::timeout(Duration::from_secs(20), notifications.recv())
        .await
        .expect("reconnect kick")
        .expect("kick");
    queue
        .enqueue(Job {
            name: JobName::Fixed(FixedJob::ReapDeadJobs),
            payload: wire::JsonObject::default(),
            key: None,
            key_mode: JobKeyMode::KeepRunAt,
            priority: 0,
            run_at: None,
            max_attempts: Some(1),
        })
        .await
        .expect("enqueue");
    tokio::time::timeout(Duration::from_secs(20), notifications.recv())
        .await
        .expect("notification after reconnect")
        .expect("notification");
}
