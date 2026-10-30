use kernel::tasks::TaskError;
use kernel::tasks::TaskWatch;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::listen::Listener;
use crate::listen::Stopped;
use crate::task_db::PostgresTasks;

impl PostgresTasks {
    pub async fn spawn_watch(
        &self,
        cancel: CancellationToken,
    ) -> Result<JoinHandle<Result<(), TaskError>>, TaskError> {
        let mut listener = Listener::connect_resilient(
            self.pool().clone(),
            &["rh_task_changed"],
            "task watch",
            cancel.clone(),
        )
        .await
        .map_err(watch_error)?;
        let events = self.events.clone();
        let finished = self.finished.clone();
        Ok(tokio::spawn(async move {
            loop {
                match listener.next().await {
                    Ok(payload) => {
                        events.send_modify(|generation| {
                            *generation = generation.saturating_add(1);
                        });
                        if let Some(payload) = payload {
                            announce(&finished, &payload);
                        }
                    }
                    Err(Stopped) => return Ok(()),
                }
            }
        }))
    }
}

fn announce(finished: &broadcast::Sender<Uuid>, payload: &str) {
    match payload.parse::<Uuid>() {
        Ok(id) => {
            if finished.send(id).is_err() {
                tracing::trace!("finished task has no listener");
            }
        }
        Err(error) => tracing::debug!(%error, "task notification names no task"),
    }
}

fn watch_error(source: sqlx::Error) -> TaskError {
    if crate::loss::unreachable(&source) {
        return TaskError::Unavailable(Box::new(source));
    }
    TaskError::Operation {
        operation: "task watch",
        source: Box::new(source),
    }
}

impl TaskWatch for PostgresTasks {
    fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.events.subscribe()
    }

    fn changed(&self) -> broadcast::Receiver<Uuid> {
        self.finished.subscribe()
    }
}
