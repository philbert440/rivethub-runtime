use std::sync::Arc;

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use kernel::tasks::TaskJobs;
use kernel::tasks::TaskStore;

use crate::engine::Engine;
use crate::error::Error;

impl<S, J> Engine<S, J>
where
    S: TaskStore + 'static,
    J: TaskJobs + 'static,
{
    pub async fn serve(self: &Arc<Self>, cancel: CancellationToken) -> Result<(), Error> {
        let wake = self.wake();
        self.notify_jobs();
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                () = wake.notified() => self.drain(&cancel).await,
                incoming = self.jobs.await_notification(cancel.clone()) => match incoming {
                    Ok(true) => self.notify_jobs(),
                    Ok(false) => break,
                    Err(error) => {
                        tracing::error!(%error, "job notification failed");
                        break;
                    }
                },
            }
        }
        self.tracker.close();
        self.tracker.wait().await;
        Ok(())
    }

    async fn drain(self: &Arc<Self>, cancel: &CancellationToken) {
        if let Err(error) = self.store.sweep(&self.settings.worker, Utc::now()).await {
            tracing::error!(%error, "task sweep failed");
            return;
        }
        loop {
            if cancel.is_cancelled() {
                return;
            }
            match self.jobs.claim(&self.selector()).await {
                Ok(None) => return,
                Ok(Some(claimed)) => {
                    let worker = Arc::clone(self);
                    let parent = cancel.clone();
                    self.tracker.spawn(async move {
                        if let Err(error) = worker.execute(claimed, &parent).await {
                            tracing::error!(%error, "task execution failed");
                        }
                    });
                }
                Err(error) => {
                    tracing::error!(%error, "task claim failed");
                    return;
                }
            }
        }
    }
}
