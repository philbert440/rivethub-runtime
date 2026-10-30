use wire::harness::{HarnessEvent, HarnessId};

use super::{ChatActor, Surface};
use crate::error::ApiError;
use crate::term::Launch;

impl ChatActor {
    pub(super) async fn fresh(&self) -> Result<bool, ApiError> {
        let harness = self.session.harness();
        if harness != HarnessId::ClaudeCode {
            return Ok(false);
        }
        Ok(self.service.stored(harness, self.session.native()).await? == Some(false))
    }

    pub(super) fn prepare(&self, launch: &mut Launch, fresh: bool) {
        if !fresh {
            launch.resume = Some(self.session.native().into());
            return;
        }
        launch.resume = None;
        if launch.startup.is_none() {
            launch.startup.clone_from(&self.startup);
        }
        if let Some(chat) = self.service.live_summary(&self.key) {
            launch.model = launch.model.take().or(chat.model);
            launch.effort = launch.effort.take().or(chat.effort);
            if let Some(cwd) = chat.cwd {
                launch.command.cwd = cwd.into();
            }
        }
    }

    pub(super) fn terminal_turn_ended(&mut self) {
        self.terminal_busy = false;
        if self.surface == Surface::Terminal {
            self.service.publish_harness(
                &self.key,
                HarnessEvent::TurnComplete {
                    session_id: self.session.clone(),
                    turn_id: None,
                    stop_reason: None,
                    usage: None,
                },
            );
        }
    }
}
