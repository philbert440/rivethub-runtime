use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use uuid::Uuid;
use wire::task::TaskStatus;

use crate::error::Error;
use crate::record::Finish;
use crate::record::TaskRecord;

#[async_trait]
pub trait TaskStore: std::fmt::Debug + Send + Sync {
    async fn insert(&self, task: &TaskRecord) -> Result<(), Error>;

    async fn load(&self, id: Uuid) -> Result<TaskRecord, Error>;

    async fn mark_running(
        &self,
        id: Uuid,
        claimed_by: &str,
        node: &str,
        now: DateTime<Utc>,
    ) -> Result<TaskRecord, Error>;

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, Error>;

    async fn sweep(&self, worker: &str, now: DateTime<Utc>) -> Result<(), Error>;

    async fn kill_queued_by_session(
        &self,
        session_key: &str,
        now: DateTime<Utc>,
    ) -> Result<(), Error>;

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, Error>;

    async fn set_pending(&self, id: Uuid, message: &str) -> Result<(), Error>;

    async fn add_harness_session(&self, id: Uuid, session: &str) -> Result<(), Error>;
}
