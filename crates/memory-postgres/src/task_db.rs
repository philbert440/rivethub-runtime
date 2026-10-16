use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::queue::ClaimSelector;
use kernel::queue::ClaimedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use kernel::tasks::Finish;
use kernel::tasks::TaskError;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use sqlx::PgConnection;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::watch;
use uuid::Uuid;
use wire::task::TaskStatus;

use crate::task_jobs::claim_job;
use crate::task_jobs::complete_job;
use crate::task_jobs::enqueue_job;
use crate::task_jobs::fail_job;
use crate::task_jobs::map_sql;
use crate::task_jobs::read_job;
use crate::task_rows::add_harness_session;
use crate::task_rows::append_permission;
use crate::task_rows::finish_task;
use crate::task_rows::insert_task;
use crate::task_rows::kill_queued_session;
use crate::task_rows::kill_task;
use crate::task_rows::load_task;
use crate::task_rows::mark_running;
use crate::task_rows::set_pending;
use crate::task_rows::sweep_tasks;

#[derive(Clone, Debug)]
pub struct PostgresTasks {
    pool: PgPool,
    worker: String,
    pub(crate) events: watch::Sender<u64>,
}

impl PostgresTasks {
    pub async fn connect(url: &str) -> Result<Self, TaskError> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(crate::ACQUIRE_TIMEOUT)
            .test_before_acquire(true)
            .connect(url)
            .await
            .map_err(|source| TaskError::Unavailable(Box::new(source)))?;
        Ok(Self::from_pool(pool))
    }

    #[must_use]
    pub fn from_pool(pool: PgPool) -> Self {
        let (events, _receiver) = watch::channel(0);
        Self {
            pool,
            worker: Uuid::new_v4().to_string(),
            events,
        }
    }

    #[must_use]
    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }
}

async fn in_transaction<T>(
    pool: &PgPool,
    body: impl AsyncFnOnce(&mut PgConnection) -> Result<T, TaskError>,
) -> Result<T, TaskError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|source| TaskError::Unavailable(Box::new(source)))?;
    set_timeouts(&mut tx).await?;
    let result = body(&mut *tx).await;
    match result {
        Ok(value) => {
            tx.commit()
                .await
                .map_err(|source| map_sql("commit", source))?;
            Ok(value)
        }
        Err(primary) => match tx.rollback().await {
            Ok(()) => Err(primary),
            Err(source) => Err(TaskError::Cleanup {
                context: "transaction",
                primary: primary.to_string(),
                cleanup: source.to_string(),
            }),
        },
    }
}

async fn set_timeouts(conn: &mut PgConnection) -> Result<(), TaskError> {
    sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(&mut *conn)
        .await
        .map_err(|source| map_sql("lock_timeout", source))?;
    sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut *conn)
        .await
        .map_err(|source| map_sql("statement_timeout", source))?;
    Ok(())
}

#[async_trait]
impl TaskStore for PostgresTasks {
    async fn insert(&self, task: &TaskRecord) -> Result<(), TaskError> {
        in_transaction(&self.pool, async |conn| insert_task(conn, task).await).await
    }

    async fn load(&self, id: Uuid) -> Result<TaskRecord, TaskError> {
        in_transaction(&self.pool, async |conn| load_task(conn, id).await).await
    }

    async fn mark_running(
        &self,
        id: Uuid,
        claimed_by: &str,
        node: &str,
        now: DateTime<Utc>,
    ) -> Result<TaskRecord, TaskError> {
        in_transaction(&self.pool, async |conn| {
            mark_running(conn, id, claimed_by, node, now).await
        })
        .await
    }

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, TaskError> {
        in_transaction(&self.pool, async |conn| finish_task(conn, id, finish).await).await
    }

    async fn sweep(&self, worker: &str, now: DateTime<Utc>) -> Result<(), TaskError> {
        in_transaction(&self.pool, async |conn| {
            sweep_tasks(conn, worker, now).await
        })
        .await
    }

    async fn kill_queued_by_session(
        &self,
        session_key: &str,
        now: DateTime<Utc>,
    ) -> Result<(), TaskError> {
        in_transaction(&self.pool, async |conn| {
            kill_queued_session(conn, session_key, now).await
        })
        .await
    }

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, TaskError> {
        in_transaction(&self.pool, async |conn| kill_task(conn, id, now).await).await
    }

    async fn set_pending(&self, id: Uuid, message: &str) -> Result<(), TaskError> {
        in_transaction(&self.pool, async |conn| {
            set_pending(conn, id, message).await
        })
        .await
    }

    async fn add_harness_session(&self, id: Uuid, session: &str) -> Result<(), TaskError> {
        in_transaction(&self.pool, async |conn| {
            add_harness_session(conn, id, session).await
        })
        .await
    }

    async fn append_permission(
        &self,
        id: Uuid,
        decision: &wire::task::TaskPermissionDecision,
    ) -> Result<(), TaskError> {
        in_transaction(&self.pool, async |conn| {
            append_permission(conn, id, decision).await
        })
        .await
    }
}

#[async_trait]
impl JobQueue for PostgresTasks {
    fn task_names(&self, role: WorkerRole) -> Vec<JobName> {
        crate::queue::PostgresQueue::task_list(role).to_vec()
    }

    async fn subscribe(&self) -> Result<Box<dyn kernel::queue::JobNotifications>, JobQueueError> {
        crate::graphile::listen(&self.pool)
            .await
            .map_err(JobQueueError::from)
    }

    async fn enqueue(&self, job: Job) -> Result<JobId, JobQueueError> {
        job_transaction(&self.pool, async |conn| enqueue_job(conn, &job).await).await
    }

    async fn claim(&self, selector: &ClaimSelector) -> Result<Option<ClaimedJob>, JobQueueError> {
        let worker = self.worker.clone();
        job_transaction(&self.pool, async move |conn| {
            claim_job(conn, selector, &worker).await
        })
        .await
    }

    async fn complete(&self, id: JobId) -> Result<(), JobQueueError> {
        let worker = self.worker.clone();
        job_transaction(&self.pool, async move |conn| {
            complete_job(conn, id, &worker).await
        })
        .await
    }

    async fn fail(&self, id: JobId, error: &str) -> Result<(), JobQueueError> {
        let worker = self.worker.clone();
        let error = error.to_owned();
        job_transaction(&self.pool, async move |conn| {
            fail_job(conn, id, &error, &worker).await
        })
        .await
    }

    async fn revive_dead(
        &self,
        name: &JobName,
        dead_before: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, JobQueueError> {
        crate::graphile::GraphileQueue::new(self.pool.clone())
            .revive_before(name, dead_before)
            .await
            .map_err(JobQueueError::from)
    }

    async fn read(&self, id: JobId) -> Result<Option<kernel::queue::StoredJob>, JobQueueError> {
        job_transaction(&self.pool, async |conn| read_job(conn, id).await).await
    }
}

async fn job_transaction<T>(
    pool: &PgPool,
    body: impl AsyncFnOnce(&mut PgConnection) -> Result<T, JobQueueError>,
) -> Result<T, JobQueueError> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|source| JobQueueError::Unavailable(Box::new(source)))?;
    if let Err(source) = sqlx::query("SET LOCAL lock_timeout = '2s'")
        .execute(&mut *tx)
        .await
    {
        return Err(map_queue_sql("lock_timeout", source));
    }
    if let Err(source) = sqlx::query("SET LOCAL statement_timeout = '5s'")
        .execute(&mut *tx)
        .await
    {
        return Err(map_queue_sql("statement_timeout", source));
    }
    let result = body(&mut tx).await;
    match result {
        Ok(value) => {
            tx.commit()
                .await
                .map_err(|source| map_queue_sql("commit", source))?;
            Ok(value)
        }
        Err(primary) => {
            if let Err(source) = tx.rollback().await {
                return Err(JobQueueError::Operation {
                    operation: "rollback",
                    source: Box::new(source),
                });
            }
            Err(primary)
        }
    }
}

fn map_queue_sql(operation: &'static str, source: sqlx::Error) -> JobQueueError {
    if crate::loss::unreachable(&source) {
        return JobQueueError::Unavailable(Box::new(source));
    }
    JobQueueError::Operation {
        operation,
        source: Box::new(source),
    }
}
