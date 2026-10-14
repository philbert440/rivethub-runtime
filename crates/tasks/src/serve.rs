use std::sync::Arc;

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use crate::engine::Engine;
use crate::error::Error;
use crate::jobs::JobWake;
use crate::jobs::TaskJobs;
use crate::sql_job::map_sql;
use crate::store::TaskStore;

impl JobWake {
    async fn recv(&mut self, cancel: &CancellationToken) -> Result<bool, Error> {
        match self {
            Self::Memory { cancel: stored } => {
                tokio::select! {
                    () = stored.cancelled() => Ok(false),
                    () = cancel.cancelled() => Ok(false),
                }
            }
            Self::Postgres {
                listener,
                cancel: stored,
            } => {
                tokio::select! {
                    () = stored.cancelled() => Ok(false),
                    () = cancel.cancelled() => Ok(false),
                    incoming = listener.recv() => incoming
                        .map(|_| true)
                        .map_err(|source| map_sql("listen", source)),
                }
            }
        }
    }
}

impl<S, J> Engine<S, J>
where
    S: TaskStore + 'static,
    J: TaskJobs + 'static,
{
    pub async fn serve(self: &Arc<Self>, cancel: CancellationToken) -> Result<(), Error> {
        let mut notes = self.jobs.wake(cancel.clone()).await?;
        let wake = self.wake();
        self.notify_jobs();
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                () = wake.notified() => self.drain(&cancel).await,
                incoming = notes.recv(&cancel) => match incoming {
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
