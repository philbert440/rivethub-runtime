use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, Utc};
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use wire::mesh::{MeshFile, MeshNode};

use super::{Registry, RegistryError};

#[derive(Debug, Clone)]
pub enum HeartbeatReport {
    Waiting,
    Updated(Arc<MeshFile>),
    Contended,
}

#[derive(Debug)]
pub struct Heartbeat {
    registry: Registry,
    local: MeshNode,
    interval: Duration,
    stale_after: TimeDelta,
    report: watch::Sender<HeartbeatReport>,
}

impl Heartbeat {
    pub fn new(
        registry: Registry,
        local: MeshNode,
        interval: Duration,
        stale_after: TimeDelta,
    ) -> Result<Self, RegistryError> {
        let stale = stale_after
            .to_std()
            .map_err(|_| RegistryError::StaleDuration)?;
        if interval.is_zero() || interval >= stale {
            return Err(RegistryError::HeartbeatInterval);
        }
        Ok(Self {
            registry,
            local,
            interval,
            stale_after,
            report: watch::channel(HeartbeatReport::Waiting).0,
        })
    }

    pub fn subscribe(&self) -> watch::Receiver<HeartbeatReport> {
        self.report.subscribe()
    }

    pub async fn run(self, shutdown: CancellationToken) -> Result<(), RegistryError> {
        let mut ticks = tokio::time::interval(self.interval);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut registered = false;
        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => return Ok(()),
                _tick = ticks.tick() => {}
            }
            let result = if registered {
                self.registry
                    .heartbeat(
                        self.local.clone(),
                        Utc::now(),
                        self.stale_after,
                        shutdown.clone(),
                    )
                    .await
            } else {
                self.registry
                    .register(self.local.clone(), Utc::now(), shutdown.clone())
                    .await
            };
            let report = match result {
                Ok(file) => {
                    registered = true;
                    HeartbeatReport::Updated(Arc::new(file))
                }
                Err(RegistryError::Busy | RegistryError::Conflict) => {
                    tracing::warn!("mesh heartbeat deferred: registry has a concurrent writer");
                    HeartbeatReport::Contended
                }
                Err(RegistryError::Cancelled) if shutdown.is_cancelled() => return Ok(()),
                Err(error) => return Err(error),
            };
            self.report.send_modify(|current| *current = report);
        }
    }
}
