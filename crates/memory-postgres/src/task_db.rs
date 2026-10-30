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
use kernel::tasks::Task;
use kernel::tasks::TaskError;
use kernel::tasks::TaskFilter;
use kernel::tasks::TaskStatus;
use kernel::tasks::TaskStore;
use sqlx::PgConnection;
use sqlx::PgPool;
use tokio::sync::broadcast;
use tokio::sync::watch;
use uuid::Uuid;

use crate::task_jobs::claim_job;
use crate::task_jobs::complete_job;
use crate::task_jobs::enqueue_job;
use crate::task_jobs::fail_job;
use crate::task_jobs::map_sql;
use crate::task_jobs::read_job;
use crate::task_jobs::release_job;
use crate::task_rows as rows;
const FINISHED_BUFFER: usize = 256;

#[derive(Clone, Debug)]
pub struct PostgresTasks {
    pool: PgPool,
    worker: String,
    pub(crate) events: watch::Sender<u64>,
    pub(crate) finished: broadcast::Sender<Uuid>,
}

impl PostgresTasks {
    pub async fn connect(url: &str) -> Result<Self, TaskError> {
        let pool = crate::open_pool(url, 8, None)
            .await
            .map_err(|source| TaskError::Unavailable(Box::new(source)))?;
        Ok(Self::from_pool(pool))
    }

    pub fn connect_lazy(url: &str) -> Result<Self, TaskError> {
        let pool = crate::lazy_pool(url, 8, None)
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
            finished: broadcast::channel(FINISHED_BUFFER).0,
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
    async fn insert(&self, task: &Task) -> Result<(), TaskError> {
        in_transaction(&self.pool, async |conn| rows::insert(conn, task).await).await
    }

    async fn load(&self, id: Uuid) -> Result<Task, TaskError> {
        in_transaction(&self.pool, async |conn| rows::load(conn, id).await).await
    }

    async fn list(&self, filter: &TaskFilter) -> Result<Vec<Task>, TaskError> {
        in_transaction(&self.pool, async |conn| rows::list(conn, filter).await).await
    }

    async fn by_child(&self, session: &str) -> Result<Option<Task>, TaskError> {
        in_transaction(&self.pool, async |conn| rows::by_child(conn, session).await).await
    }

    async fn begin(&self, id: Uuid, node: &str, now: DateTime<Utc>) -> Result<bool, TaskError> {
        in_transaction(&self.pool, async |conn| {
            rows::begin(conn, id, node, now).await
        })
        .await
    }

    async fn attach(
        &self,
        id: Uuid,
        child: &str,
        harness: kernel::tasks::Harness,
    ) -> Result<(), TaskError> {
        in_transaction(&self.pool, async |conn| {
            rows::attach(conn, id, child, harness).await
        })
        .await
    }

    async fn flip(&self, id: Uuid, status: TaskStatus) -> Result<bool, TaskError> {
        in_transaction(&self.pool, async |conn| rows::flip(conn, id, status).await).await
    }

    async fn alive(&self, ids: &[Uuid], now: DateTime<Utc>) -> Result<(), TaskError> {
        if ids.is_empty() {
            return Ok(());
        }
        in_transaction(&self.pool, async |conn| rows::alive(conn, ids, now).await).await
    }

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, TaskError> {
        in_transaction(&self.pool, async |conn| {
            rows::finish(conn, id, finish).await
        })
        .await
    }

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, TaskError> {
        in_transaction(&self.pool, async |conn| rows::kill(conn, id, now).await).await
    }

    async fn orphans(&self, node: &str, now: DateTime<Utc>) -> Result<Vec<Uuid>, TaskError> {
        in_transaction(&self.pool, async |conn| {
            rows::orphans(conn, node, now).await
        })
        .await
    }

    async fn reap(
        &self,
        stale_before: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, TaskError> {
        in_transaction(&self.pool, async |conn| {
            rows::reap(conn, stale_before, now).await
        })
        .await
    }

    async fn expire(&self, now: DateTime<Utc>) -> Result<Vec<Uuid>, TaskError> {
        in_transaction(&self.pool, async |conn| rows::expire(conn, now).await).await
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

    async fn release(&self, id: JobId) -> Result<(), JobQueueError> {
        let worker = self.worker.clone();
        job_transaction(&self.pool, async move |conn| {
            release_job(conn, id, &worker).await
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
