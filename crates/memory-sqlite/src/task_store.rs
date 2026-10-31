use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::tasks::Finish;
use kernel::tasks::Task;
use kernel::tasks::TaskError;
use kernel::tasks::TaskFilter;
use kernel::tasks::TaskStatus;
use kernel::tasks::TaskStore;
use rusqlite::Connection;
use uuid::Uuid;

use crate::SqliteStore;
use crate::task_rows as rows;

#[async_trait]
impl TaskStore for SqliteStore {
    async fn insert(&self, task: &Task) -> Result<(), TaskError> {
        let task = task.clone();
        call(self, move |conn| rows::insert(conn, &task)).await
    }

    async fn load(&self, id: Uuid) -> Result<Task, TaskError> {
        call(self, move |conn| rows::load(conn, id)).await
    }

    async fn list(&self, filter: &TaskFilter) -> Result<Vec<Task>, TaskError> {
        let filter = filter.clone();
        call(self, move |conn| rows::list(conn, &filter)).await
    }

    async fn by_child(&self, session: &str) -> Result<Option<Task>, TaskError> {
        let session = session.to_owned();
        call(self, move |conn| rows::by_child(conn, &session)).await
    }

    async fn begin(&self, id: Uuid, node: &str, now: DateTime<Utc>) -> Result<bool, TaskError> {
        let node = node.to_owned();
        call(self, move |conn| rows::begin(conn, id, &node, now)).await
    }

    async fn attach(
        &self,
        id: Uuid,
        child: &str,
        harness: kernel::tasks::Harness,
    ) -> Result<(), TaskError> {
        let child = child.to_owned();
        call(self, move |conn| rows::attach(conn, id, &child, harness)).await
    }

    async fn flip(&self, id: Uuid, status: TaskStatus) -> Result<bool, TaskError> {
        call(self, move |conn| rows::flip(conn, id, status)).await
    }

    async fn alive(&self, ids: &[Uuid], now: DateTime<Utc>) -> Result<(), TaskError> {
        let ids = ids.to_vec();
        call(self, move |conn| rows::alive(conn, &ids, now)).await
    }

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, TaskError> {
        let finish = finish.clone();
        call(self, move |conn| rows::finish(conn, id, &finish)).await
    }

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, TaskError> {
        call(self, move |conn| rows::kill(conn, id, now)).await
    }

    async fn orphans(&self, node: &str, now: DateTime<Utc>) -> Result<Vec<Uuid>, TaskError> {
        let node = node.to_owned();
        call(self, move |conn| {
            rows::fail_open(conn, "node = ?1", &node, "its gateway restarted", now)
        })
        .await
    }

    async fn reap(
        &self,
        stale_before: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, TaskError> {
        let bound = rows::stamp(stale_before);
        call(self, move |conn| {
            rows::fail_open(
                conn,
                "(alive_at IS NULL OR alive_at < ?1)",
                &bound,
                "its node stopped running it",
                now,
            )
        })
        .await
    }

    async fn expire(&self, now: DateTime<Utc>) -> Result<Vec<Uuid>, TaskError> {
        call(self, move |conn| rows::expire(conn, now)).await
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
