use async_trait::async_trait;
use kernel::tasks::TaskError;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskWatch;
use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Null;
use tokio::sync::watch;

use crate::SqliteError;
use crate::SqliteStore;

pub(crate) fn register(conn: &Connection, events: watch::Sender<u64>) -> Result<(), SqliteError> {
    conn.create_scalar_function("rh_task_done", 0, FunctionFlags::SQLITE_UTF8, move |_| {
        events.send_modify(|generation| *generation = generation.saturating_add(1));
        Ok(Null)
    })
    .map_err(SqliteError::query("register task watch"))
}

impl TaskWatch for SqliteStore {
    fn subscribe(&self) -> watch::Receiver<u64> {
        self.task_events.subscribe()
    }
}

#[async_trait]
impl TaskQuery for SqliteStore {
    async fn list(&self, filter: &TaskListFilter) -> Result<Vec<TaskRecord>, TaskError> {
        let filter = filter.clone();
        crate::task_store::call(self, move |conn| {
            crate::task_rows::list_tasks(conn, &filter)
        })
        .await
    }
}
