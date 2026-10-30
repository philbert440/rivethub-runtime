use wire::harness::{HarnessEvent, SessionDriver, TerminalApproval};

use super::{ChatActor, Surface};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TerminalSignal {
    Submitted,
    Typed,
    Progressed,
    Approval(TerminalApproval),
    TurnEnded,
    Exited,
}

impl TerminalSignal {
    pub(crate) fn typed(bytes: &[u8]) -> Self {
        if bytes.contains(&b'\r') {
            Self::Submitted
        } else {
            Self::Typed
        }
    }
}

impl ChatActor {
    pub(super) fn terminal_signal(&mut self, signal: TerminalSignal) {
        match signal {
            TerminalSignal::Submitted => {
                self.approval = None;
                if self.surface == Surface::Terminal {
                    self.terminal_busy = true;
                }
            }
            TerminalSignal::Typed | TerminalSignal::Progressed => self.approval = None,
            TerminalSignal::Approval(approval) => {
                if self.surface == Surface::Terminal {
                    self.approval = Some(approval);
                }
            }
            TerminalSignal::TurnEnded => {
                self.approval = None;
                self.terminal_turn_ended();
            }
            TerminalSignal::Exited => {
                if self.surface != Surface::Terminal || self.service.pty_running(&self.key) {
                    return;
                }
                if self.terminal_busy {
                    self.terminal_turn_ended();
                }
                self.terminal_busy = false;
                self.approval = None;
                self.surface = Surface::Idle;
            }
        }
    }

    pub(super) fn announce(&mut self, running: bool) {
        let (driver, busy) = match self.surface {
            Surface::Chat => (Some(SessionDriver::Chat), running),
            Surface::Terminal => (Some(SessionDriver::Terminal), self.terminal_busy),
            Surface::Idle => (None, false),
        };
        let event = HarnessEvent::Driver {
            session_id: self.session.clone(),
            driver,
            busy,
            queued: self.queued.iter().map(|queued| queued.text.clone()).collect(),
            approval: self.approval.clone(),
        };
        if self.announced.as_ref() == Some(&event) {
            return;
        }
        self.announced = Some(event.clone());
        self.service.publish_harness(&self.key, event);
    }
}
