use async_trait::async_trait;
use kernel::tasks::TaskError;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskWatch;
use rusqlite::Connection;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::Null;
use tokio::sync::broadcast;
use tokio::sync::watch;
use uuid::Uuid;

use crate::SqliteError;
use crate::SqliteStore;

pub(crate) fn register(
    conn: &Connection,
    events: watch::Sender<u64>,
    finished: broadcast::Sender<Uuid>,
) -> Result<(), SqliteError> {
    conn.create_scalar_function("rh_task_done", 1, FunctionFlags::SQLITE_UTF8, move |ctx| {
        events.send_modify(|generation| *generation = generation.saturating_add(1));
        announce(&finished, &ctx.get::<String>(0)?);
        Ok(Null)
    })
    .map_err(SqliteError::query("register task watch"))
}

fn announce(finished: &broadcast::Sender<Uuid>, id: &str) {
    match id.parse::<Uuid>() {
        Ok(id) => {
            if finished.send(id).is_err() {
                tracing::trace!("finished task has no listener");
            }
        }
        Err(error) => tracing::debug!(%error, "finished task row has no task id"),
    }
}

impl TaskWatch for SqliteStore {
    fn subscribe(&self) -> watch::Receiver<u64> {
        self.task_events.subscribe()
    }

    fn finished(&self) -> broadcast::Receiver<Uuid> {
        self.task_finished.subscribe()
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
