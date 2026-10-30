use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::tasks::Finish;
use kernel::tasks::TaskError;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use rusqlite::Connection;
use uuid::Uuid;
use wire::task::TaskPermissionDecision;
use wire::task::TaskStatus;

use crate::SqliteStore;

#[async_trait]
impl TaskStore for SqliteStore {
    async fn insert(&self, task: &TaskRecord) -> Result<(), TaskError> {
        let task = task.clone();
        call(self, move |conn| crate::task_rows::insert_task(conn, &task)).await
    }

    async fn load(&self, id: Uuid) -> Result<TaskRecord, TaskError> {
        call(self, move |conn| crate::task_rows::load_task(conn, id)).await
    }

    async fn mark_running(
        &self,
        id: Uuid,
        claimed_by: &str,
        node: &str,
        now: DateTime<Utc>,
    ) -> Result<TaskRecord, TaskError> {
        let claimed_by = claimed_by.to_owned();
        let node = node.to_owned();
        call(self, move |conn| {
            crate::task_rows::mark_running(conn, id, &claimed_by, &node, now)
        })
        .await
    }

    async fn release_running(&self, id: Uuid) -> Result<(), TaskError> {
        call(self, move |conn| {
            crate::task_rows::release_running(conn, id)
        })
        .await
    }

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, TaskError> {
        let finish = finish.clone();
        call(self, move |conn| {
            crate::task_rows::finish_task(conn, id, &finish)
        })
        .await
    }

    async fn sweep(&self, worker: &str, now: DateTime<Utc>) -> Result<(), TaskError> {
        let worker = worker.to_owned();
        call(self, move |conn| {
            crate::task_rows::sweep_tasks(conn, &worker, now)
        })
        .await
    }

    async fn heartbeat(&self, id: Uuid, worker: &str, now: DateTime<Utc>) -> Result<(), TaskError> {
        let worker = worker.to_owned();
        call(self, move |conn| {
            crate::task_rows::heartbeat(conn, id, &worker, now)
        })
        .await
    }

    async fn kill_queued_by_session(
        &self,
        session_key: &str,
        now: DateTime<Utc>,
    ) -> Result<(), TaskError> {
        let session_key = session_key.to_owned();
        call(self, move |conn| {
            crate::task_rows::kill_queued(conn, &session_key, now)
        })
        .await
    }

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, TaskError> {
        call(self, move |conn| crate::task_rows::kill_task(conn, id, now)).await
    }

    async fn set_pending(&self, id: Uuid, message: &str) -> Result<(), TaskError> {
        let message = message.to_owned();
        call(self, move |conn| {
            crate::task_rows::set_pending(conn, id, &message)
        })
        .await
    }

    async fn append_permission(
        &self,
        id: Uuid,
        decision: &TaskPermissionDecision,
    ) -> Result<(), TaskError> {
        let decision = decision.clone();
        call(self, move |conn| {
            crate::task_rows::append_permission(conn, id, &decision)
        })
        .await
    }

    async fn add_harness_session(&self, id: Uuid, session: &str) -> Result<(), TaskError> {
        let session = session.to_owned();
        call(self, move |conn| {
            crate::task_rows::add_harness(conn, id, &session)
        })
        .await
    }
}

pub(crate) async fn call<T, F>(store: &SqliteStore, body: F) -> Result<T, TaskError>
where
    T: Send + 'static,
    F: FnOnce(&mut Connection) -> Result<T, TaskError> + Send + 'static,
{
    match store.with_conn(move |conn| Ok(body(conn))).await {
        Ok(result) => result,
        Err(error) => Err(TaskError::Unavailable(Box::new(error))),
    }
}
