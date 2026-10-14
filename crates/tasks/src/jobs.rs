use async_trait::async_trait;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::payload::TaskJobPayload;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    pub name: JobName,
    pub payload: TaskJobPayload,
    pub key: String,
    pub key_mode: JobKeyMode,
    pub priority: i16,
    pub max_attempts: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimSelector {
    pub names: Vec<String>,
    pub only_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedJob {
    pub id: i64,
    pub identifier: String,
    pub payload_text: String,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJob {
    pub id: i64,
    pub identifier: String,
    pub payload: TaskJobPayload,
    pub key: Option<String>,
    pub priority: i16,
    pub max_attempts: i32,
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[derive(Debug)]
pub enum JobWake {
    Memory {
        cancel: CancellationToken,
    },
    Postgres {
        listener: sqlx::postgres::PgListener,
        cancel: CancellationToken,
    },
}

#[async_trait]
pub trait TaskJobs: std::fmt::Debug + Send + Sync {
    async fn enqueue(&self, job: &NewJob) -> Result<i64, Error>;

    async fn claim(&self, selector: &ClaimSelector) -> Result<Option<ClaimedJob>, Error>;

    async fn complete(&self, id: i64) -> Result<(), Error>;

    async fn fail(&self, id: i64, error: &str) -> Result<(), Error>;

    async fn read(&self, id: i64) -> Result<Option<StoredJob>, Error>;

    async fn wake(&self, cancel: CancellationToken) -> Result<JobWake, Error>;
}

pub(crate) fn key_mode_text(mode: JobKeyMode) -> &'static str {
    match mode {
        JobKeyMode::KeepRunAt => "preserve_run_at",
        JobKeyMode::Replace => "replace",
    }
}
