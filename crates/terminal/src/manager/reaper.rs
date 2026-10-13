use std::sync::Arc;
use std::time::Duration;

use kernel::terminal::TerminalId;
use tokio_util::sync::CancellationToken;

use crate::error::Error;
use crate::manager::TerminalManager;
use crate::manager::registry::KillPlan;
use crate::manager::registry::Runtime;
use crate::pty;
use tokio::time::Instant;

const EVICTION_QUIET: Duration = Duration::from_millis(500);
const KILL_GRACE: Duration = Duration::from_millis(3_000);
const DETACH_KILL_GRACE: Duration = Duration::from_millis(1_000);
const REAPER_TICK: Duration = Duration::from_millis(1_000);

impl TerminalManager {
    pub fn run_tasks(self: &Arc<Self>, shutdown: CancellationToken) {
        let taken = self.inner.detach_rx.lock().take();
        if let Some(mut detach_rx) = taken {
            let manager = Arc::clone(self);
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                loop {
                    let detached = tokio::select! {
                        () = shutdown.cancelled() => break,
                        detached = detach_rx.recv() => detached,
                    };
                    let Some(id) = detached else {
                        break;
                    };
                    manager.mark_detached(&id);
                }
            });
        }
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    () = tokio::time::sleep(REAPER_TICK) => manager.sweep(),
                }
            }
        });
    }

    pub(super) fn enforce_cap(&self, running: usize) -> Result<Option<TerminalId>, Error> {
        let max = self.inner.settings.limits.max_ptys;
        if running < max {
            return Ok(None);
        }
        let now = Instant::now();
        let victim = {
            let state = self.inner.state.lock();
            let victim = state
                .iter()
                .filter_map(|(id, runtime)| {
                    let Runtime::Pty {
                        live: Some(live),
                        exit_code: None,
                        ..
                    } = runtime
                    else {
                        return None;
                    };
                    if live.viewers > 0 || live.hangup_sent {
                        return None;
                    }
                    let quiet_since = live.last_output?;
                    if now.duration_since(quiet_since) < EVICTION_QUIET {
                        return None;
                    }
                    Some((live.last_activity, id.clone()))
                })
                .min_by_key(|(last_activity, _)| *last_activity)
                .map(|(_, id)| id);
            drop(state);
            victim
        };
        match victim {
            Some(id) => Ok(Some(id)),
            None => Err(Error::PtyLimit { max }),
        }
    }

    pub(super) fn evict(&self, id: &TerminalId) {
        let pid = {
            let mut state = self.inner.state.lock();
            match state.get_mut(id) {
                Some(Runtime::Pty {
                    live: Some(live), ..
                }) => {
                    live.hangup_sent = true;
                    live.pid
                }
                _ => None,
            }
        };
        let Some(pid) = pid else { return };
        if let Err(error) = pty::signal(pid, nix::sys::signal::Signal::SIGHUP) {
            tracing::warn!("terminal {id} eviction SIGHUP to {pid} failed ({error})");
            return;
        }
        self.escalate(id.clone(), pid, DETACH_KILL_GRACE);
    }

    pub async fn kill(&self, id: &TerminalId) -> Result<(), Error> {
        let plan = {
            let mut state = self.inner.state.lock();
            match state.get_mut(id) {
                None => KillPlan::Unknown,
                Some(Runtime::Herdr { session, .. }) => KillPlan::StopHerdr(session.clone()),
                Some(Runtime::Pty {
                    live: Some(live),
                    exit_code: None,
                    ..
                }) => {
                    live.hangup_sent = true;
                    match live.pid {
                        Some(pid) => KillPlan::Hangup { pid },
                        None => KillPlan::Remove,
                    }
                }
                Some(Runtime::Pty { .. }) => KillPlan::Remove,
            }
        };
        match plan {
            KillPlan::Unknown => Err(Error::UnknownTerminal {
                terminal: id.clone(),
            }),
            KillPlan::Remove => {
                self.inner.state.lock().remove(id);
                self.unbind_session(id);
                Ok(())
            }
            KillPlan::StopHerdr(session) => {
                session.stop().await?;
                self.inner.state.lock().remove(id);
                self.unbind_session(id);
                Ok(())
            }
            KillPlan::Hangup { pid } => {
                pty::signal(pid, nix::sys::signal::Signal::SIGHUP)?;
                self.escalate(id.clone(), pid, KILL_GRACE);
                Ok(())
            }
        }
    }

    fn escalate(&self, id: TerminalId, pid: u32, grace: Duration) {
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            let alive = matches!(
                inner.state.lock().get(&id),
                Some(Runtime::Pty {
                    live: Some(_),
                    exit_code: None,
                    ..
                })
            );
            if alive && let Err(error) = pty::signal(pid, nix::sys::signal::Signal::SIGKILL) {
                tracing::warn!("terminal {id} SIGKILL escalation to {pid} failed ({error})");
            }
        });
    }

    fn sweep(&self) {
        let now = Instant::now();
        let limits = &self.inner.settings.limits;
        let detached_ttl = Duration::from_millis(limits.detached_ttl_ms);
        let idle_ttl = Duration::from_millis(limits.idle_ttl_ms);
        let reap_after = if limits.session_gc_ms > 0 {
            Duration::from_millis(limits.session_gc_ms)
                .max(Duration::from_millis(limits.exit_linger_ms))
        } else {
            Duration::from_millis(limits.exit_linger_ms)
        };
        let mut reaped: Vec<TerminalId> = Vec::new();
        let mut kills: Vec<(TerminalId, u32)> = Vec::new();
        {
            let state = self.inner.state.lock();
            for (id, runtime) in state.iter() {
                let Runtime::Pty {
                    live,
                    exit_code,
                    exited_at,
                    ..
                } = runtime
                else {
                    continue;
                };
                if let (None, Some(_)) = (live.as_ref(), exit_code.as_ref()) {
                    if let Some(at) = exited_at.as_ref()
                        && now.duration_since(*at) >= reap_after
                    {
                        reaped.push(id.clone());
                    }
                    continue;
                }
                if let Some(live) = live.as_ref()
                    && exit_code.is_none()
                    && live.viewers == 0
                {
                    let detached = now.duration_since(live.detached_since) >= detached_ttl;
                    let idle = idle_ttl > Duration::ZERO
                        && now.duration_since(live.last_activity) >= idle_ttl;
                    if (detached || idle)
                        && let Some(pid) = live.pid
                    {
                        kills.push((id.clone(), pid));
                    }
                }
            }
        }
        for id in reaped {
            if self.inner.state.lock().remove(&id).is_some() {
                self.unbind_session(&id);
            }
        }
        for (id, pid) in kills {
            let pending = {
                let mut state = self.inner.state.lock();
                match state.get_mut(&id) {
                    Some(Runtime::Pty {
                        live: Some(live), ..
                    }) if !live.hangup_sent => {
                        live.hangup_sent = true;
                        true
                    }
                    _ => false,
                }
            };
            if !pending {
                continue;
            }
            if let Err(error) = pty::signal(pid, nix::sys::signal::Signal::SIGHUP) {
                tracing::warn!("terminal {id} reaper SIGHUP to {pid} failed ({error})");
                continue;
            }
            self.escalate(id, pid, DETACH_KILL_GRACE);
        }
    }
}
