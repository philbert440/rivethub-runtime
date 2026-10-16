use chrono::Utc;
use kernel::queue::ClaimSelector;
use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::tasks::TaskJobPayload;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use memory_postgres::PostgresTasks;
use sqlx::PgPool;
use sqlx::Row;
use std::str::FromStr;
use uuid::Uuid;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskPromptMode;
use wire::task::TaskSpec;
use wire::task::TaskStatus;

#[expect(
    clippy::expect_used,
    reason = "a task payload is a typed struct; serde failure means the fixture is wrong"
)]
fn job(name: JobName, task_id: &str, key: &str) -> Job {
    Job {
        name,
        payload: wire::JsonObject::from_serialize(&TaskJobPayload {
            task_id: task_id.to_string(),
        })
        .expect("task payload"),
        key: Some(key.to_string()),
        key_mode: JobKeyMode::Replace,
        priority: 0,
        run_at: None,
        max_attempts: Some(1),
    }
}

struct Dumped {
    task: String,
    queue: Option<String>,
    payload: String,
    key: Option<String>,
    priority: i64,
    max_attempts: i64,
    attempts: i64,
}

#[expect(
    clippy::expect_used,
    reason = "a queue helper fails the case when a row or claim does not come back"
)]
async fn dumped(pool: &PgPool, task_id: &str) -> Dumped {
    let row = sqlx::query(
        "SELECT t.identifier AS task, q.queue_name AS queue, j.payload::text AS payload, j.key, \
                j.priority::bigint AS priority, j.max_attempts::bigint AS max_attempts, \
                j.attempts::bigint AS attempts \
           FROM graphile_worker._private_jobs j \
           JOIN graphile_worker._private_tasks t ON t.id = j.task_id \
           LEFT JOIN graphile_worker._private_job_queues q ON q.id = j.job_queue_id \
          WHERE j.payload->>'taskId' = $1",
    )
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("job row");
    Dumped {
        task: row.try_get("task").expect("task"),
        queue: row.try_get("queue").expect("queue"),
        payload: row.try_get("payload").expect("payload"),
        key: row.try_get("key").expect("key"),
        priority: row.try_get("priority").expect("priority"),
        max_attempts: row.try_get("max_attempts").expect("max_attempts"),
        attempts: row.try_get("attempts").expect("attempts"),
    }
}

#[expect(
    clippy::expect_used,
    reason = "a queue helper fails the case when a row or claim does not come back"
)]
fn assert_contract(row: &Dumped, task_name: &str, task_id: &str, key: &str) {
    assert_eq!(row.task, task_name);
    assert!(row.queue.is_none());
    assert_eq!(row.priority, 0);
    assert_eq!(row.max_attempts, 1);
    assert_eq!(row.attempts, 0);
    assert_eq!(row.key.as_deref(), Some(key));
    let payload: TaskJobPayload = serde_json::from_str(&row.payload).expect("payload");
    assert_eq!(payload.task_id, task_id);
    assert!(!row.payload.contains("\"node\""));
}

#[expect(
    clippy::expect_used,
    reason = "a queue helper fails the case when a row or claim does not come back"
)]
async fn count_key(pool: &PgPool, key: &str) -> i64 {
    let row = sqlx::query(
        "SELECT count(*)::bigint AS n FROM graphile_worker._private_jobs WHERE key = $1",
    )
    .bind(key)
    .fetch_one(pool)
    .await
    .expect("count");
    row.try_get("n").expect("n")
}

#[expect(
    clippy::expect_used,
    reason = "the helper only receives job names the suite constructs"
)]
fn selector(name: &str, id: JobId) -> ClaimSelector {
    ClaimSelector {
        names: vec![JobName::from_str(name).expect("job name")],
        only_id: Some(id),
    }
}

#[expect(
    clippy::expect_used,
    reason = "a queue helper fails the case when a row or claim does not come back"
)]
async fn park(pool: &PgPool, id: JobId) {
    let updated = sqlx::query(
        "UPDATE graphile_worker._private_jobs \
         SET run_at = now() + interval '1 day' \
         WHERE id = $1 AND locked_at IS NULL",
    )
    .bind(i64::from(id))
    .execute(pool)
    .await
    .expect("park");
    assert_eq!(
        updated.rows_affected(),
        1,
        "job {id} was claimed before it could be parked"
    );
}

#[expect(
    clippy::expect_used,
    reason = "a queue helper fails the case when a row or claim does not come back"
)]
async fn release(pool: &PgPool, id: JobId) {
    sqlx::query("UPDATE graphile_worker._private_jobs SET run_at = now() WHERE id = $1")
        .bind(i64::from(id))
        .execute(pool)
        .await
        .expect("release");
}

fn queued_record(id: Uuid, goal: &str) -> TaskRecord {
    let session = format!("task:{id}");
    TaskRecord {
        id,
        goal: goal.to_string(),
        context_refs: Vec::new(),
        acceptance_criteria: Vec::new(),
        spec: TaskSpec {
            task_id: id.to_string(),
            agent_id: "worker".to_string(),
            goal: goal.to_string(),
            resolved_context: Vec::new(),
            acceptance_criteria: Vec::new(),
            budget: TaskBudget::default(),
            tools: None,
            working_dir: None,
            model: None,
            effort: None,
            system_prompt_append: None,
            isolation: None,
            prompt_mode: Some(TaskPromptMode::Task),
            exclude_tools: None,
            resume_message: None,
            session: session.clone(),
            permission_decisions: None,
            preset_id: None,
            preset_name: None,
            audit_only: None,
            extra: wire::Extra::default(),
        },
        executor: TaskExecutorKind::ChatLoop,
        executor_target: None,
        requested_by: None,
        agent_id: "worker".to_string(),
        origin: TaskOrigin::Chat,
        parent_task_id: None,
        chain_depth: 0,
        budget: TaskBudget::default(),
        node_affinity: None,
        usage: None,
        status: TaskStatus::Queued,
        attempt: 0,
        max_attempts: 1,
        result: None,
        error: None,
        conversation_id: None,
        session_key: Some(session),
        pending_message: None,
        claimed_by: None,
        harness_session_ids: Vec::new(),
        created_at: Utc::now(),
        started_at: None,
        last_heartbeat_at: None,
        completed_at: None,
        duration_ms: None,
        eval: None,
        eval_attempt: 0,
    }
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn graphile_rows_match_the_task_queue_contract(pool: PgPool) {
    assert!(
        !testkit::queue_present(&pool).await.expect("probe"),
        "the migrated database has no graphile schema until the fixture is installed"
    );
    let postgres = PostgresTasks::from_pool(pool.clone());
    let error = postgres
        .enqueue(job(
            JobName::Fixed(FixedJob::RunTask),
            "absent",
            "task-absent",
        ))
        .await
        .expect_err("absent");
    assert!(matches!(error, JobQueueError::Absent));
    testkit::install_graphile_worker(&pool)
        .await
        .expect("graphile fixture");
    assert!(
        testkit::queue_present(&pool).await.expect("installed"),
        "the fixture installs graphile_worker"
    );
    exercise(pool).await;
}

async fn exercise(pool: PgPool) {
    let postgres = PostgresTasks::from_pool(pool.clone());
    let marker = Uuid::now_v7().to_string();
    submitted(&pool, &postgres).await;
    named_jobs(&pool, &postgres, &marker).await;
    replaced(&pool, &postgres, &marker).await;
    completed(&pool, &postgres, &marker).await;
    failed_job(&pool, &postgres, &marker).await;
}

#[expect(
    clippy::expect_used,
    reason = "a queue helper fails the case when a row or claim does not come back"
)]
async fn submitted(pool: &PgPool, postgres: &PostgresTasks) {
    let id = Uuid::now_v7();
    postgres
        .insert(&queued_record(id, "ship it"))
        .await
        .expect("insert");
    let key = format!("task:{id}");
    let queued = postgres
        .enqueue(job(
            JobName::Fixed(FixedJob::RunTask),
            &id.to_string(),
            &key,
        ))
        .await
        .expect("enqueue");
    park(pool, queued).await;
    let loaded = postgres.load(id).await.expect("load");
    assert_eq!(loaded.goal, "ship it");
    assert_eq!(loaded.status, TaskStatus::Queued);
    assert_eq!(loaded.session_key.as_deref(), Some(key.as_str()));
    assert_contract(
        &dumped(pool, &id.to_string()).await,
        "run-task",
        &id.to_string(),
        &key,
    );
    postgres.set_pending(id, "resume").await.expect("pending");
    let running = postgres
        .mark_running(id, "worker-1", "local", Utc::now())
        .await
        .expect("claim row");
    assert_eq!(running.pending_message.as_deref(), Some("resume"));
    assert_eq!(running.status, TaskStatus::Running);
    let stored = postgres.load(id).await.expect("stored");
    assert_eq!(stored.pending_message, None);
    assert_eq!(
        stored.started_at,
        Some(running.started_at.expect("started"))
    );
}

#[expect(
    clippy::expect_used,
    reason = "a queue helper fails the case when a row or claim does not come back"
)]
async fn named_jobs(pool: &PgPool, postgres: &PostgresTasks, marker: &str) {
    let remote = format!("remote-{marker}");
    let remote_key = format!("task-remote-{marker}");
    let queued = postgres
        .enqueue(job(
            JobName::RunTaskOn {
                node: "edge".to_string(),
            },
            &remote,
            &remote_key,
        ))
        .await
        .expect("remote");
    park(pool, queued).await;
    assert_contract(
        &dumped(pool, &remote).await,
        "run-task:edge",
        &remote,
        &remote_key,
    );
    let beat = format!("beat-{marker}");
    let beat_key = format!("heartbeat-worker-1-{marker}");
    let queued = postgres
        .enqueue(job(
            JobName::Heartbeat {
                agent: "worker".to_string(),
                index: 1,
            },
            &beat,
            &beat_key,
        ))
        .await
        .expect("heartbeat");
    park(pool, queued).await;
    assert_contract(
        &dumped(pool, &beat).await,
        "heartbeat-worker-1",
        &beat,
        &beat_key,
    );
    assert!(
        postgres
            .claim(&selector("run-task", queued))
            .await
            .expect("leave heartbeat")
            .is_none()
    );
}

#[expect(
    clippy::expect_used,
    reason = "a queue helper fails the case when a row or claim does not come back"
)]
async fn replaced(pool: &PgPool, postgres: &PostgresTasks, marker: &str) {
    let key = format!("task-replace-{marker}");
    let first = postgres
        .enqueue(job(
            JobName::Fixed(FixedJob::RunTask),
            &format!("first-{marker}"),
            &key,
        ))
        .await
        .expect("first");
    park(pool, first).await;
    let second = postgres
        .enqueue(job(
            JobName::Fixed(FixedJob::RunTask),
            &format!("second-{marker}"),
            &key,
        ))
        .await
        .expect("second");
    park(pool, second).await;
    assert_eq!(count_key(pool, &key).await, 1);
    assert_contract(
        &dumped(pool, &format!("second-{marker}")).await,
        "run-task",
        &format!("second-{marker}"),
        &key,
    );
}

#[expect(
    clippy::expect_used,
    reason = "a queue helper fails the case when a row or claim does not come back"
)]
async fn completed(pool: &PgPool, postgres: &PostgresTasks, marker: &str) {
    let task_id = format!("complete-{marker}");
    let key = format!("task-complete-{marker}");
    let queued = postgres
        .enqueue(job(JobName::Fixed(FixedJob::RunTask), &task_id, &key))
        .await
        .expect("complete enqueue");
    park(pool, queued).await;
    release(pool, queued).await;
    let claimed = postgres
        .claim(&selector("run-task", queued))
        .await
        .expect("claim")
        .expect("claimed");
    postgres.complete(claimed.id).await.expect("complete");
    assert!(postgres.read(claimed.id).await.expect("read").is_none());
}

#[expect(
    clippy::expect_used,
    reason = "a queue helper fails the case when a row or claim does not come back"
)]
async fn failed_job(pool: &PgPool, postgres: &PostgresTasks, marker: &str) {
    let task_id = format!("fail-{marker}");
    let key = format!("task-fail-{marker}");
    let queued = postgres
        .enqueue(job(JobName::Fixed(FixedJob::RunTask), &task_id, &key))
        .await
        .expect("fail enqueue");
    park(pool, queued).await;
    release(pool, queued).await;
    let claimed = postgres
        .claim(&selector("run-task", queued))
        .await
        .expect("claim")
        .expect("claimed");
    postgres.fail(claimed.id, "boom").await.expect("fail");
    assert!(
        postgres
            .claim(&selector("run-task", queued))
            .await
            .expect("reclaim")
            .is_none()
    );
    let stored = postgres.read(queued).await.expect("read").expect("stored");
    assert_eq!(stored.attempts, 1);
    assert_eq!(stored.last_error.as_deref(), Some("boom"));
}
