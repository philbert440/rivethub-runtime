use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::tasks::ClaimSelector;
use kernel::tasks::ClaimedJob;
use kernel::tasks::Finish;
use kernel::tasks::NewJob;
use kernel::tasks::StoredJob;
use kernel::tasks::TaskError;
use kernel::tasks::TaskJobs;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use sqlx::PgConnection;
use sqlx::PgPool;
use sqlx::postgres::PgListener;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::task::TaskStatus;

use crate::task_jobs::claim_job;
use crate::task_jobs::complete_job;
use crate::task_jobs::enqueue_job;
use crate::task_jobs::fail_job;
use crate::task_jobs::map_sql;
use crate::task_jobs::read_job;
use crate::task_rows::add_harness_session;
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
    listen: Arc<Mutex<Option<PgListener>>>,
}

impl PostgresTasks {
    pub async fn connect(url: &str) -> Result<Self, TaskError> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(url)
            .await
            .map_err(|source| TaskError::Unavailable(Box::new(source)))?;
        Ok(Self::from_pool(pool))
    }

    #[must_use]
    pub fn from_pool(pool: PgPool) -> Self {
        Self {
            pool,
            listen: Arc::new(Mutex::new(None)),
        }
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
}

#[async_trait]
impl TaskJobs for PostgresTasks {
    async fn enqueue(&self, job: &NewJob) -> Result<i64, TaskError> {
        in_transaction(&self.pool, async |conn| enqueue_job(conn, job).await).await
    }

    async fn claim(&self, selector: &ClaimSelector) -> Result<Option<ClaimedJob>, TaskError> {
        in_transaction(&self.pool, async |conn| claim_job(conn, selector).await).await
    }

    async fn complete(&self, id: i64) -> Result<(), TaskError> {
        in_transaction(&self.pool, async |conn| complete_job(conn, id).await).await
    }

    async fn fail(&self, id: i64, error: &str) -> Result<(), TaskError> {
        in_transaction(&self.pool, async |conn| fail_job(conn, id, error).await).await
    }

    async fn read(&self, id: i64) -> Result<Option<StoredJob>, TaskError> {
        in_transaction(&self.pool, async |conn| read_job(conn, id).await).await
    }

    async fn await_notification(&self, cancel: CancellationToken) -> Result<bool, TaskError> {
        let parked = self.listen.lock().await.take();
        let mut listener = if let Some(listener) = parked {
            listener
        } else {
            let mut created = PgListener::connect_with(&self.pool)
                .await
                .map_err(|source| map_sql("listen", source))?;
            created
                .listen("jobs:insert")
                .await
                .map_err(|source| map_sql("listen", source))?;
            created
        };
        let result = tokio::select! {
            () = cancel.cancelled() => Ok(false),
            incoming = listener.recv() => incoming.map(|_| true).map_err(|source| map_sql("listen", source)),
        };
        *self.listen.lock().await = Some(listener);
        result
    }
}
