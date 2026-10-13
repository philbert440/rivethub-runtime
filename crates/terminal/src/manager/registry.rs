use std::sync::Arc;

use kernel::terminal::TerminalId;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::herdr::HerdrSession;
use crate::manager::Backing;
use crate::manager::SpawnSpec;
use crate::manager::TerminalManager;
use crate::manager::TerminalRecord;
use crate::manager::TerminalState;
use crate::pty;
use crate::scrollback::Scrollback;
use crate::session::RosterKey;
use crate::session::SessionKey;

pub(super) struct LivePty {
    pub(super) master: Box<dyn portable_pty::MasterPty + Send>,
    pub(super) pid: Option<u32>,
    pub(super) writer: mpsc::Sender<Vec<u8>>,
    pub(super) output: broadcast::Sender<Arc<[u8]>>,
    pub(super) viewers: u32,
    pub(super) detached_since: Instant,
    pub(super) last_activity: Instant,
    pub(super) last_output: Option<Instant>,
    pub(super) hangup_sent: bool,
}

pub(super) enum Runtime {
    Pty {
        scrollback: Scrollback,
        fields: RecordFields,
        live: Option<LivePty>,
        exit_code: Option<u32>,
        exited_at: Option<Instant>,
    },
    Herdr {
        fields: RecordFields,
        session: HerdrSession,
    },
}

pub(super) struct RecordFields {
    pub(super) id: TerminalId,
    pub(super) session: Option<SessionKey>,
    pub(super) roster_key: RosterKey,
    pub(super) argv: Vec<String>,
    pub(super) size: kernel::terminal::TermSize,
    pub(super) backing: Backing,
}

impl RecordFields {
    pub(super) fn new(spec: &SpawnSpec, id: TerminalId, backing: Backing) -> Self {
        Self {
            id,
            session: spec.session.clone(),
            roster_key: spec.roster_key.clone(),
            argv: spec.argv.clone(),
            size: spec.size,
            backing,
        }
    }

    fn record(&self, pid: Option<u32>, state: TerminalState, attached: u32) -> TerminalRecord {
        TerminalRecord {
            id: self.id.clone(),
            session: self.session.clone(),
            roster_key: self.roster_key.clone(),
            argv: self.argv.clone(),
            pid,
            size: self.size,
            state,
            attached,
            backing: self.backing,
        }
    }
}

pub(super) enum KillPlan {
    Unknown,
    Remove,
    Hangup { pid: u32 },
    StopHerdr(HerdrSession),
}

impl TerminalManager {
    pub(super) fn is_running(&self, id: &TerminalId) -> bool {
        matches!(
            self.inner.state.lock().get(id),
            Some(Runtime::Pty {
                live: Some(_),
                exit_code: None,
                ..
            })
        )
    }

    pub(super) fn count_running(&self) -> usize {
        self.inner
            .state
            .lock()
            .values()
            .filter(|runtime| {
                matches!(
                    runtime,
                    Runtime::Pty {
                        live: Some(_),
                        exit_code: None,
                        ..
                    }
                )
            })
            .count()
    }

    pub(super) fn fresh_id(&self) -> TerminalId {
        loop {
            let candidate = TerminalId::new(format!("pty-{}", short_hex()));
            if !self.inner.state.lock().contains_key(&candidate) {
                return candidate;
            }
        }
    }

    pub(super) fn bind_session(&self, id: &TerminalId, session: Option<&SessionKey>) {
        if let Some(session) = session {
            self.inner
                .by_session
                .lock()
                .insert(session.clone(), id.clone());
        }
    }

    pub(super) fn unbind_session(&self, id: &TerminalId) {
        self.inner.by_session.lock().retain(|_, bound| bound != id);
    }

    pub(super) fn bound_terminal(&self, session: &SessionKey) -> Option<TerminalId> {
        self.inner.by_session.lock().get(session).cloned()
    }

    pub(super) fn pty_record(
        fields: &RecordFields,
        live: Option<&LivePty>,
        exit_code: Option<&u32>,
    ) -> TerminalRecord {
        fields.record(
            live.and_then(|live| live.pid),
            record_state(live, exit_code),
            live.map_or(0, |live| live.viewers),
        )
    }

    pub fn list(&self) -> Vec<TerminalRecord> {
        self.inner
            .state
            .lock()
            .values()
            .map(|runtime| match runtime {
                Runtime::Herdr { fields, .. } => fields.record(None, TerminalState::Running, 0),
                Runtime::Pty {
                    fields,
                    live,
                    exit_code,
                    ..
                } => Self::pty_record(fields, live.as_ref(), exit_code.as_ref()),
            })
            .collect()
    }

    pub fn close(&self) {
        let mut state = self.inner.state.lock();
        for runtime in state.values_mut() {
            let Runtime::Pty {
                live: Some(live), ..
            } = runtime
            else {
                continue;
            };
            if live.hangup_sent {
                continue;
            }
            live.hangup_sent = true;
            if let Some(pid) = live.pid
                && let Err(error) = pty::signal(pid, nix::sys::signal::Signal::SIGHUP)
            {
                tracing::warn!("terminal shutdown SIGHUP to {pid} failed ({error})");
            }
        }
    }
}

pub(super) fn record_state(live: Option<&LivePty>, exit_code: Option<&u32>) -> TerminalState {
    match (live, exit_code) {
        (Some(_), _) => TerminalState::Running,
        (None, Some(code)) => TerminalState::Exited { code: *code },
        (None, None) => TerminalState::Exited { code: 0 },
    }
}

fn short_hex() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect()
}
