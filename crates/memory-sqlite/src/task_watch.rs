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
    conn.create_scalar_function(
        "rh_task_changed",
        1,
        FunctionFlags::SQLITE_UTF8,
        move |ctx| {
            events.send_modify(|generation| *generation = generation.saturating_add(1));
            announce(&finished, &ctx.get::<String>(0)?);
            Ok(Null)
        },
    )
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

    fn changed(&self) -> broadcast::Receiver<Uuid> {
        self.task_finished.subscribe()
    }
}
