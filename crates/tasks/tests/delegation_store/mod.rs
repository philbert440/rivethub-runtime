use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kernel::tasks::{TaskError, TaskRecord, TaskStore};
use testkit::TaskMemory;
use tokio::sync::watch;
use uuid::Uuid;
use wire::task::{TaskPermissionDecision, TaskStatus};

#[derive(Debug)]
pub(super) struct NotifyStore {
    pub(super) inner: TaskMemory,
    pub(super) inserted: watch::Sender<Option<Uuid>>,
}

#[async_trait]
impl TaskStore for NotifyStore {
    async fn insert(&self, task: &TaskRecord) -> Result<(), TaskError> {
        self.inner.insert(task).await?;
        let _previous = self.inserted.send_replace(Some(task.id));
        Ok(())
    }

    async fn load(&self, id: Uuid) -> Result<TaskRecord, TaskError> {
        self.inner.load(id).await
    }

    async fn mark_running(
        &self,
        id: Uuid,
        claimed_by: &str,
        node: &str,
        now: DateTime<Utc>,
    ) -> Result<TaskRecord, TaskError> {
        self.inner.mark_running(id, claimed_by, node, now).await
    }

    async fn finish(&self, id: Uuid, finish: &kernel::tasks::Finish) -> Result<bool, TaskError> {
        self.inner.finish(id, finish).await
    }

    async fn sweep(&self, worker: &str, now: DateTime<Utc>) -> Result<(), TaskError> {
        self.inner.sweep(worker, now).await
    }

    async fn kill_queued_by_session(
        &self,
        session_key: &str,
        now: DateTime<Utc>,
    ) -> Result<(), TaskError> {
        self.inner.kill_queued_by_session(session_key, now).await
    }

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, TaskError> {
        self.inner.kill(id, now).await
    }

    async fn set_pending(&self, id: Uuid, message: &str) -> Result<(), TaskError> {
        self.inner.set_pending(id, message).await
    }

    async fn add_harness_session(&self, id: Uuid, session: &str) -> Result<(), TaskError> {
        self.inner.add_harness_session(id, session).await
    }

    async fn append_permission(
        &self,
        id: Uuid,
        decision: &TaskPermissionDecision,
    ) -> Result<(), TaskError> {
        self.inner.append_permission(id, decision).await
    }
}
