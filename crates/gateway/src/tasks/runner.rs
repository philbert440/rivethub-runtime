use std::sync::Arc;

use chrono::Utc;
use kernel::queue::ClaimSelector;
use kernel::queue::JobName;
use kernel::queue::JobNotifications;
use kernel::queue::JobQueueError;
use kernel::tasks::LIVENESS;
use kernel::tasks::TaskStatus;
use kernel::tasks::stale_before;
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use super::Tasks;
use super::child;
use crate::Gateway;

pub async fn serve(gateway: Arc<Gateway>, cancel: CancellationToken) {
    let Some(tasks) = gateway.tasks().cloned() else {
        return;
    };
    let mut orphans_failed = false;
    let mut jobs: Option<Box<dyn JobNotifications>> = None;
    let mut changes = tasks.watch.changed();
    let mut beat = tokio::time::interval(LIVENESS);
    let children = TaskTracker::new();
    let stopping = cancel.child_token();
    loop {
        tokio::select! {
            () = cancel.cancelled() => break,
            _ = beat.tick() => {
                if !orphans_failed {
                    orphans_failed = tasks.fail_orphans().await;
                }
                if jobs.is_none() {
                    jobs = tasks.listen().await;
                }
                tasks.beat().await;
                tasks.drain(&gateway, &children, &stopping).await;
            }
            incoming = next_job(&mut jobs) => match incoming {
                Ok(()) => tasks.drain(&gateway, &children, &stopping).await,
                Err(error) => {
                    tracing::warn!(%error, "task job notifications failed");
                    jobs = None;
                }
            },
            noticed = changes.recv() => match noticed {
                Ok(id) => tasks.changed(&gateway, id).await,
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => changes = tasks.watch.changed(),
            },
        }
    }
    stopping.cancel();
    children.close();
    children.wait().await;
}

async fn next_job(jobs: &mut Option<Box<dyn JobNotifications>>) -> Result<(), JobQueueError> {
    match jobs {
        Some(jobs) => jobs.recv().await,
        None => std::future::pending().await,
    }
}

impl Tasks {
    async fn fail_orphans(&self) -> bool {
        match self.store.orphans(&self.node, Utc::now()).await {
            Ok(orphaned) => {
                if !orphaned.is_empty() {
                    tracing::warn!(
                        count = orphaned.len(),
                        "failed the tasks this node was running before it restarted"
                    );
                }
                true
            }
            Err(error) => {
                tracing::warn!(%error, "this node's tasks from before it restarted are not failed yet");
                false
            }
        }
    }

    async fn listen(&self) -> Option<Box<dyn JobNotifications>> {
        match self.jobs.subscribe().await {
            Ok(jobs) => Some(jobs),
            Err(error) => {
                tracing::warn!(%error, "task job notifications are not open yet");
                None
            }
        }
    }

    async fn beat(&self) {
        let now = Utc::now();
        if let Err(error) = self.store.alive(&self.running_ids(), now).await {
            tracing::warn!(%error, "task liveness was not recorded");
        }
        if let Some(stale) = stale_before(now) {
            match self.store.reap(stale, now).await {
                Ok(reaped) if !reaped.is_empty() => {
                    tracing::warn!(
                        count = reaped.len(),
                        "failed tasks whose node stopped running them"
                    );
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "stale tasks were not reaped"),
            }
        }
        match self.store.expire(now).await {
            Ok(expired) if !expired.is_empty() => {
                tracing::warn!(
                    count = expired.len(),
                    "timed out queued tasks that no node started before their deadline"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "unclaimed tasks were not timed out"),
        }
    }

    async fn drain(
        self: &Arc<Self>,
        gateway: &Arc<Gateway>,
        children: &TaskTracker,
        stopping: &CancellationToken,
    ) {
        let selector = ClaimSelector {
            names: vec![JobName::RunTaskOn {
                node: self.node.clone(),
            }],
            only_id: None,
        };
        loop {
            if stopping.is_cancelled() {
                return;
            }
            match self.jobs.claim(&selector).await {
                Ok(Some(claimed)) => {
                    children.spawn(kernel::tenancy::Scope::carry(child::run(
                        Arc::clone(gateway),
                        Arc::clone(self),
                        claimed,
                        stopping.child_token(),
                    )));
                }
                Ok(None) => return,
                Err(error) => {
                    tracing::warn!(%error, "task claim failed");
                    return;
                }
            }
        }
    }

    async fn changed(self: &Arc<Self>, gateway: &Arc<Gateway>, id: Uuid) {
        let task = match self.store.load(id).await {
            Ok(task) => task,
            Err(error) => {
                tracing::debug!(%error, %id, "changed task was not read");
                return;
            }
        };
        if task.status == TaskStatus::Killed {
            self.stop(id);
        }
        if !task.status.terminal() {
            return;
        }
        let hosted = task.parent.session.as_deref().is_some_and(|parent| {
            gateway
                .terminal()
                .is_some_and(|terminal| terminal.live_chat_of(parent).is_some())
        });
        if !hosted {
            self.waits.forget(id);
        } else if self.waits.deliver(id) {
            tokio::spawn(kernel::tenancy::Scope::carry(
                Arc::clone(self).deliver_late(Arc::clone(gateway), task),
            ));
        }
    }
}
