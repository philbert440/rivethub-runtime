#![expect(
    clippy::expect_used,
    reason = "allow-expect-in-tests covers only #[test] functions; helpers panic when a fixture or a local stub fails to set up"
)]
#![expect(
    clippy::panic,
    reason = "lane cleanup and connect failures must panic without the sqlx error, which embeds the database url"
)]

mod common;

use std::collections::BTreeMap;

use chrono::Utc;
use common::settings;
use common::task;
use config::Environment;
use kernel::queue::FixedJob;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use sqlx::PgPool;
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;
use tasks::ClaimSelector;
use tasks::Engine;
use tasks::Error;
use tasks::NewJob;
use tasks::Postgres;
use tasks::TaskJobPayload;
use tasks::TaskJobs;
use tasks::TaskStore;
use url::Url;
use uuid::Uuid;
use wire::task::TaskStatus;

fn job(name: JobName, task_id: &str, key: &str) -> NewJob {
    NewJob {
        name,
        payload: TaskJobPayload {
            task_id: task_id.to_string(),
        },
        key: key.to_string(),
        key_mode: JobKeyMode::Replace,
        priority: 0,
        max_attempts: 1,
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

fn selector(name: &str, id: i64) -> ClaimSelector {
    ClaimSelector {
        names: vec![name.to_string()],
        only_id: Some(id),
    }
}

struct Residue {
    url: Option<String>,
    jobs: Vec<i64>,
    tasks: Vec<Uuid>,
    keys: Vec<String>,
}

impl Residue {
    fn ephemeral() -> Self {
        Self {
            url: None,
            jobs: Vec::new(),
            tasks: Vec::new(),
            keys: Vec::new(),
        }
    }

    fn lane(url: String) -> Self {
        Self {
            url: Some(url),
            jobs: Vec::new(),
            tasks: Vec::new(),
            keys: Vec::new(),
        }
    }

    fn job(&mut self, id: i64) {
        self.jobs.push(id);
    }

    fn task(&mut self, id: Uuid) {
        self.tasks.push(id);
    }

    fn key(&mut self, key: String) {
        self.keys.push(key);
    }
}

impl Drop for Residue {
    fn drop(&mut self) {
        let Some(url) = self.url.clone() else {
            return;
        };
        let jobs = std::mem::take(&mut self.jobs);
        let tasks = std::mem::take(&mut self.tasks);
        let keys = std::mem::take(&mut self.keys);
        let handle = std::thread::Builder::new()
            .name("tasks-pg-cleanup".to_string())
            .spawn(move || remove_residue(&url, &jobs, &tasks, &keys))
            .expect("cleanup thread");
        handle.join().expect("cleanup finished");
    }
}

fn remove_residue(url: &str, jobs: &[i64], tasks: &[Uuid], keys: &[String]) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("cleanup runtime");
    runtime.block_on(async move {
        let Ok(pool) = PgPoolOptions::new().max_connections(1).connect(url).await else {
            panic!("cleanup connection failed");
        };
        sqlx::query(
            "DELETE FROM graphile_worker._private_jobs \
             WHERE id = ANY($1::bigint[]) OR key = ANY($2::text[])",
        )
        .bind(jobs)
        .bind(keys)
        .execute(&pool)
        .await
        .expect("delete jobs");
        sqlx::query("DELETE FROM rh_tasks WHERE id = ANY($1::uuid[])")
            .bind(tasks)
            .execute(&pool)
            .await
            .expect("delete tasks");
    });
}

async fn park(pool: &PgPool, id: i64) {
    let updated = sqlx::query(
        "UPDATE graphile_worker._private_jobs \
         SET run_at = now() + interval '1 day' \
         WHERE id = $1 AND locked_at IS NULL",
    )
    .bind(id)
    .execute(pool)
    .await
    .expect("park");
    assert_eq!(
        updated.rows_affected(),
        1,
        "job {id} was claimed before it could be parked"
    );
}

async fn release(pool: &PgPool, id: i64) {
    sqlx::query("UPDATE graphile_worker._private_jobs SET run_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .expect("release");
}

async fn job_id(pool: &PgPool, task_id: &str) -> i64 {
    let row = sqlx::query(
        "SELECT j.id FROM graphile_worker._private_jobs j WHERE j.payload->>'taskId' = $1",
    )
    .bind(task_id)
    .fetch_one(pool)
    .await
    .expect("job id");
    row.try_get("id").expect("id")
}

fn lane_url() -> String {
    Environment::from_process()
        .text("RIVETOS_PG_URL")
        .filter(|url| !url.is_empty())
        .expect("lane postgres url")
        .to_string()
}

async fn connect_lane(url: &str) -> PgPool {
    let Ok(pool) = PgPoolOptions::new().max_connections(4).connect(url).await else {
        panic!("lane postgres connection failed");
    };
    pool
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn graphile_rows_match_the_task_queue_contract(pool: PgPool) {
    if testkit::queue_present(&pool).await.expect("probe") {
        exercise(pool, Residue::ephemeral()).await;
        return;
    }
    let postgres = Postgres::from_pool(pool);
    let error = postgres
        .enqueue(&job(
            JobName::Fixed(FixedJob::RunTask),
            "absent",
            "task-absent",
        ))
        .await
        .expect_err("absent");
    assert!(matches!(error, Error::QueueAbsent));
    let url = lane_url();
    let lane = connect_lane(&url).await;
    assert!(testkit::queue_present(&lane).await.expect("lane probe"));
    exercise(lane, Residue::lane(url)).await;
}

async fn exercise(pool: PgPool, mut residue: Residue) {
    let postgres = Postgres::from_pool(pool.clone());
    let marker = Uuid::now_v7().to_string();
    submitted(&pool, &postgres, &mut residue).await;
    named_jobs(&pool, &postgres, &mut residue, &marker).await;
    replaced(&pool, &postgres, &mut residue, &marker).await;
    completed(&pool, &postgres, &mut residue, &marker).await;
    failed_job(&pool, &postgres, &mut residue, &marker).await;
}

async fn submitted(pool: &PgPool, postgres: &Postgres, residue: &mut Residue) {
    let base = Url::parse("http://127.0.0.1/").expect("url");
    let engine = Engine::new(
        postgres.clone(),
        postgres.clone(),
        common::directory(base),
        settings(None, BTreeMap::new()),
    )
    .expect("engine");
    let id = engine.submit(task("ship it")).await.expect("submit");
    residue.task(id);
    let queued = job_id(pool, &id.to_string()).await;
    residue.job(queued);
    residue.key(format!("task:{id}"));
    park(pool, queued).await;
    let loaded = engine.load(id).await.expect("load");
    assert_eq!(loaded.goal, "ship it");
    assert_eq!(loaded.status, TaskStatus::Queued);
    assert_eq!(
        loaded.session_key.as_deref(),
        Some(format!("task:{id}")).as_deref()
    );
    assert_contract(
        &dumped(pool, &id.to_string()).await,
        "run-task",
        &id.to_string(),
        &format!("task:{id}"),
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

async fn named_jobs(pool: &PgPool, postgres: &Postgres, residue: &mut Residue, marker: &str) {
    let remote = format!("remote-{marker}");
    let remote_key = format!("task-remote-{marker}");
    let queued = postgres
        .enqueue(&job(
            JobName::RunTaskOn {
                node: "edge".to_string(),
            },
            &remote,
            &remote_key,
        ))
        .await
        .expect("remote");
    residue.job(queued);
    residue.key(remote_key.clone());
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
        .enqueue(&job(
            JobName::Heartbeat {
                agent: "worker".to_string(),
                index: 1,
            },
            &beat,
            &beat_key,
        ))
        .await
        .expect("heartbeat");
    residue.job(queued);
    residue.key(beat_key.clone());
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

async fn replaced(pool: &PgPool, postgres: &Postgres, residue: &mut Residue, marker: &str) {
    let key = format!("task-replace-{marker}");
    let first = postgres
        .enqueue(&job(
            JobName::Fixed(FixedJob::RunTask),
            &format!("first-{marker}"),
            &key,
        ))
        .await
        .expect("first");
    residue.job(first);
    residue.key(key.clone());
    park(pool, first).await;
    let second = postgres
        .enqueue(&job(
            JobName::Fixed(FixedJob::RunTask),
            &format!("second-{marker}"),
            &key,
        ))
        .await
        .expect("second");
    residue.job(second);
    park(pool, second).await;
    assert_eq!(count_key(pool, &key).await, 1);
    assert_contract(
        &dumped(pool, &format!("second-{marker}")).await,
        "run-task",
        &format!("second-{marker}"),
        &key,
    );
}

async fn completed(pool: &PgPool, postgres: &Postgres, residue: &mut Residue, marker: &str) {
    let task_id = format!("complete-{marker}");
    let key = format!("task-complete-{marker}");
    let queued = postgres
        .enqueue(&job(JobName::Fixed(FixedJob::RunTask), &task_id, &key))
        .await
        .expect("complete enqueue");
    residue.job(queued);
    residue.key(key);
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

async fn failed_job(pool: &PgPool, postgres: &Postgres, residue: &mut Residue, marker: &str) {
    let task_id = format!("fail-{marker}");
    let key = format!("task-fail-{marker}");
    let queued = postgres
        .enqueue(&job(JobName::Fixed(FixedJob::RunTask), &task_id, &key))
        .await
        .expect("fail enqueue");
    residue.job(queued);
    residue.key(key);
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
