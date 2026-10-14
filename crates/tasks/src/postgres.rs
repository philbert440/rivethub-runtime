use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use sqlx::PgConnection;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::task::TaskStatus;

use crate::error::Error;
use crate::jobs::ClaimSelector;
use crate::jobs::ClaimedJob;
use crate::jobs::JobWake;
use crate::jobs::NewJob;
use crate::jobs::StoredJob;
use crate::jobs::TaskJobs;
use crate::record::Finish;
use crate::record::TaskRecord;
use crate::sql_job::claim_job;
use crate::sql_job::complete_job;
use crate::sql_job::enqueue_job;
use crate::sql_job::fail_job;
use crate::sql_job::map_sql;
use crate::sql_job::read_job;
use crate::sql_task::add_harness_session;
use crate::sql_task::finish_task;
use crate::sql_task::insert_task;
use crate::sql_task::kill_queued_session;
use crate::sql_task::kill_task;
use crate::sql_task::load_task;
use crate::sql_task::mark_running;
use crate::sql_task::set_pending;
use crate::sql_task::sweep_tasks;
use crate::store::TaskStore;

#[derive(Clone, Debug)]
pub struct Postgres {
    pool: PgPool,
}

impl Postgres {
    pub async fn connect(url: &str) -> Result<Self, Error> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(url)
            .await
            .map_err(|source| Error::Connection(Box::new(source)))?;
        Ok(Self { pool })
    }

    #[must_use]
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn in_transaction<T>(
    pool: &PgPool,
    body: impl AsyncFnOnce(&mut PgConnection) -> Result<T, Error>,
) -> Result<T, Error> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|source| Error::Connection(Box::new(source)))?;
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
            Err(source) => Err(Error::Cleanup {
                context: "transaction",
                primary: primary.to_string(),
                cleanup: source.to_string(),
            }),
        },
    }
}

async fn set_timeouts(conn: &mut PgConnection) -> Result<(), Error> {
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
impl TaskStore for Postgres {
    async fn insert(&self, task: &TaskRecord) -> Result<(), Error> {
        in_transaction(&self.pool, async |conn| insert_task(conn, task).await).await
    }

    async fn load(&self, id: Uuid) -> Result<TaskRecord, Error> {
        in_transaction(&self.pool, async |conn| load_task(conn, id).await).await
    }

    async fn mark_running(
        &self,
        id: Uuid,
        claimed_by: &str,
        node: &str,
        now: DateTime<Utc>,
    ) -> Result<TaskRecord, Error> {
        in_transaction(&self.pool, async |conn| {
            mark_running(conn, id, claimed_by, node, now).await
        })
        .await
    }

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, Error> {
        in_transaction(&self.pool, async |conn| finish_task(conn, id, finish).await).await
    }

    async fn sweep(&self, worker: &str, now: DateTime<Utc>) -> Result<(), Error> {
        in_transaction(&self.pool, async |conn| {
            sweep_tasks(conn, worker, now).await
        })
        .await
    }

    async fn kill_queued_by_session(
        &self,
        session_key: &str,
        now: DateTime<Utc>,
    ) -> Result<(), Error> {
        in_transaction(&self.pool, async |conn| {
            kill_queued_session(conn, session_key, now).await
        })
        .await
    }

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, Error> {
        in_transaction(&self.pool, async |conn| kill_task(conn, id, now).await).await
    }

    async fn set_pending(&self, id: Uuid, message: &str) -> Result<(), Error> {
        in_transaction(&self.pool, async |conn| {
            set_pending(conn, id, message).await
        })
        .await
    }

    async fn add_harness_session(&self, id: Uuid, session: &str) -> Result<(), Error> {
        in_transaction(&self.pool, async |conn| {
            add_harness_session(conn, id, session).await
        })
        .await
    }
}

#[async_trait]
impl TaskJobs for Postgres {
    async fn enqueue(&self, job: &NewJob) -> Result<i64, Error> {
        in_transaction(&self.pool, async |conn| enqueue_job(conn, job).await).await
    }

    async fn claim(&self, selector: &ClaimSelector) -> Result<Option<ClaimedJob>, Error> {
        in_transaction(&self.pool, async |conn| claim_job(conn, selector).await).await
    }

    async fn complete(&self, id: i64) -> Result<(), Error> {
        in_transaction(&self.pool, async |conn| complete_job(conn, id).await).await
    }

    async fn fail(&self, id: i64, error: &str) -> Result<(), Error> {
        in_transaction(&self.pool, async |conn| fail_job(conn, id, error).await).await
    }

    async fn read(&self, id: i64) -> Result<Option<StoredJob>, Error> {
        in_transaction(&self.pool, async |conn| read_job(conn, id).await).await
    }

    async fn wake(&self, cancel: CancellationToken) -> Result<JobWake, Error> {
        let mut listener = sqlx::postgres::PgListener::connect_with(&self.pool)
            .await
            .map_err(|source| map_sql("listen", source))?;
        listener
            .listen("jobs:insert")
            .await
            .map_err(|source| map_sql("listen", source))?;
        Ok(JobWake::Postgres { listener, cancel })
    }
}
