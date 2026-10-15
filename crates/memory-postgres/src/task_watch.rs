use async_trait::async_trait;
use kernel::tasks::TaskError;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskWatch;
use sqlx::postgres::PgListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::task_db::PostgresTasks;
use crate::task_query::list_tasks;

const DONE_CHANNEL: &str = "ros_task_done";

impl PostgresTasks {
    pub async fn spawn_watch(
        &self,
        cancel: CancellationToken,
    ) -> Result<JoinHandle<Result<(), TaskError>>, TaskError> {
        let mut listener = PgListener::connect_with(self.pool())
            .await
            .map_err(watch_error)?;
        listener.listen(DONE_CHANNEL).await.map_err(watch_error)?;
        let events = self.events.clone();
        Ok(tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Ok(()),
                    incoming = listener.recv() => {
                        incoming.map_err(watch_error)?;
                        events.send_modify(|generation| {
                            *generation = generation.saturating_add(1);
                        });
                    }
                }
            }
        }))
    }
}

fn watch_error(source: sqlx::Error) -> TaskError {
    TaskError::Operation {
        operation: "task watch",
        source: Box::new(source),
    }
}

impl TaskWatch for PostgresTasks {
    fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.events.subscribe()
    }
}

#[async_trait]
impl TaskQuery for PostgresTasks {
    async fn list(&self, filter: &TaskListFilter) -> Result<Vec<TaskRecord>, TaskError> {
        list_tasks(self.pool(), filter).await
    }
}
