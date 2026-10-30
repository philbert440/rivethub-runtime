use std::path::PathBuf;

use strum::IntoEnumIterator as _;
use tokio::sync::broadcast;
use wire::harness::{
    HarnessEvent, HarnessId, HarnessSessionStatus, HarnessSessionSummary, RosterCommand,
};

use kernel::harness::ToolCallUpdate;
use kernel::preview::{LiveActivity, Preview};
use wire::session_id::SessionId;

use super::{LaunchCommand, TerminalService};

const HARNESS_EVENT_BUFFER: usize = 256;
pub(crate) const REGISTRY_EVENT_BUFFER: usize = 256;

#[derive(Debug, Clone)]
pub(crate) struct RegistryEvent {
    pub(crate) key: String,
    pub(crate) event: HarnessEvent,
}

fn serves(command: &LaunchCommand, harness: HarnessId) -> bool {
    command.harness == Some(harness) || command.id == RosterCommand::from(harness).to_string()
}

impl TerminalService {
    pub(crate) fn roster(&self) -> Vec<HarnessId> {
        HarnessId::iter()
            .filter(|harness| {
                self.options
                    .commands
                    .iter()
                    .any(|command| serves(command, *harness))
            })
            .collect()
    }

    pub(crate) fn roster_cwd(&self, harness: HarnessId) -> Option<PathBuf> {
        self.options
            .commands
            .iter()
            .find(|command| serves(command, harness))
            .map(|command| command.cwd.clone())
    }

    pub(crate) fn live_summary(&self, key: &str) -> Option<HarnessSessionSummary> {
        self.summaries.read().get(key).cloned()
    }

    pub(crate) fn live_summaries(
        &self,
        harness: HarnessId,
    ) -> Vec<(String, HarnessSessionSummary)> {
        self.summaries
            .read()
            .iter()
            .filter(|(_, summary)| summary.harness_id == harness)
            .map(|(key, summary)| (key.clone(), summary.clone()))
            .collect()
    }

    pub(crate) fn subscribe_harness(&self, key: &str) -> broadcast::Receiver<HarnessEvent> {
        if let Some(sender) = self.harness_events.read().get(key) {
            return sender.subscribe();
        }
        self.harness_events
            .write()
            .entry(key.to_owned())
            .or_insert_with(|| broadcast::channel(HARNESS_EVENT_BUFFER).0)
            .subscribe()
    }

    pub(crate) fn release_harness(&self, key: &str) {
        let mut senders = self.harness_events.write();
        if senders
            .get(key)
            .is_some_and(|sender| sender.receiver_count() == 0)
        {
            senders.remove(key);
        }
        drop(senders);
    }

    pub(crate) fn publish_harness(&self, key: &str, event: HarnessEvent) {
        self.follow(key, &event);
        self.hold_approvals(key, &event);
        let sender = self.harness_events.read().get(key).cloned();
        if let Some(sender) = sender
            && sender.send(event).is_err()
        {
            tracing::trace!("harness event has no live session socket");
        }
    }

    pub(crate) fn pending_approvals(&self, key: &str) -> Vec<HarnessEvent> {
        self.approvals.read().get(key).cloned().unwrap_or_default()
    }

    fn hold_approvals(&self, key: &str, event: &HarnessEvent) {
        let mut approvals = self.approvals.write();
        match event {
            HarnessEvent::ApprovalRequest { .. } => {
                approvals
                    .entry(key.to_owned())
                    .or_default()
                    .push(event.clone());
            }
            HarnessEvent::ApprovalResolved { request_id, .. } => {
                if let Some(waiting) = approvals.get_mut(key) {
                    waiting.retain(|held| {
                        !matches!(held, HarnessEvent::ApprovalRequest { request_id: id, .. } if id == request_id)
                    });
                }
            }
            HarnessEvent::TurnComplete { .. } | HarnessEvent::Error { .. } => {
                approvals.remove(key);
            }
            HarnessEvent::SessionUpdated { status, .. }
                if *status != HarnessSessionStatus::Active =>
            {
                approvals.remove(key);
            }
            HarnessEvent::AssistantDelta { .. }
            | HarnessEvent::ReasoningDelta { .. }
            | HarnessEvent::ToolUse { .. }
            | HarnessEvent::ToolUpdate { .. }
            | HarnessEvent::ToolResult { .. }
            | HarnessEvent::SessionCreated { .. }
            | HarnessEvent::Plan { .. }
            | HarnessEvent::SessionUpdated { .. }
            | HarnessEvent::Transcript { .. }
            | HarnessEvent::Status(_)
            | HarnessEvent::Prompt { .. } => {}
        }
    }

    pub(crate) fn tool_updated(&self, key: &str, update: &ToolCallUpdate) {
        if let Some(live) = self.activity.write().get_mut(key) {
            live.tool_updated(
                &update.tool_call_id,
                update.title.as_deref(),
                update.input.as_ref(),
            );
        }
    }

    pub(crate) fn live_preview(&self, key: &str) -> Option<Preview> {
        self.activity
            .read()
            .get(key)
            .and_then(LiveActivity::preview)
    }

    fn follow(&self, key: &str, event: &HarnessEvent) {
        let now = chrono::Utc::now();
        let mut activity = self.activity.write();
        match event {
            HarnessEvent::AssistantDelta { text, .. } => {
                activity.entry(key.to_owned()).or_default().wrote(text, now);
            }
            HarnessEvent::ToolUse {
                tool_call_id,
                name,
                input,
                ..
            } => activity.entry(key.to_owned()).or_default().tool_started(
                tool_call_id,
                name,
                input,
                now,
            ),
            HarnessEvent::ApprovalRequest {
                request_id,
                name,
                input,
                ..
            } => activity
                .entry(key.to_owned())
                .or_default()
                .approval_requested(request_id, name, input, now),
            HarnessEvent::ToolResult { tool_call_id, .. } => {
                if let Some(live) = activity.get_mut(key) {
                    live.tool_finished(tool_call_id);
                }
            }
            HarnessEvent::ApprovalResolved { request_id, .. } => {
                if let Some(live) = activity.get_mut(key) {
                    live.approval_resolved(request_id);
                }
            }
            HarnessEvent::TurnComplete { .. } | HarnessEvent::Error { .. } => {
                activity.remove(key);
            }
            HarnessEvent::SessionUpdated { status, .. }
                if *status != HarnessSessionStatus::Active =>
            {
                activity.remove(key);
            }
            HarnessEvent::SessionUpdated { .. }
            | HarnessEvent::ReasoningDelta { .. }
            | HarnessEvent::ToolUpdate { .. }
            | HarnessEvent::SessionCreated { .. }
            | HarnessEvent::Plan { .. }
            | HarnessEvent::Transcript { .. }
            | HarnessEvent::Status(_)
            | HarnessEvent::Prompt { .. } => {}
        }
    }

    pub(crate) fn subscribe_registry(&self) -> broadcast::Receiver<RegistryEvent> {
        self.registry_events.subscribe()
    }

    pub(crate) fn publish_registry(&self, key: String, event: HarnessEvent) {
        if self
            .registry_events
            .send(RegistryEvent { key, event })
            .is_err()
        {
            tracing::trace!("registry event has no live harness socket");
        }
    }

    pub(crate) fn set_live_status(
        &self,
        key: &str,
        session: &SessionId,
        status: HarnessSessionStatus,
    ) {
        let settled = status != HarnessSessionStatus::Active;
        let blocked = match self.summaries.write().get_mut(key) {
            Some(summary) => {
                summary.status = status;
                summary.updated_at = chrono::Utc::now();
                if settled {
                    summary.blocked = None;
                }
                summary.blocked
            }
            None => None,
        };
        let reported = if settled { Some(false) } else { blocked };
        self.publish_update(key, session, status, reported);
    }

    pub(crate) fn chose(&self, key: &str, option: kernel::harness::SessionOption) {
        if let Some(summary) = self.summaries.write().get_mut(key) {
            match option {
                kernel::harness::SessionOption::Model(model) => summary.model = Some(model),
                kernel::harness::SessionOption::Effort(level) => {
                    summary.effort = Some(level.to_string());
                }
                kernel::harness::SessionOption::Mode(_) => {}
            }
        }
    }

    pub(crate) fn set_blocked(&self, key: &str, session: &SessionId, blocked: bool) {
        let status = match self.summaries.write().get_mut(key) {
            Some(summary) if summary.blocked.unwrap_or(false) != blocked => {
                summary.blocked = blocked.then_some(true);
                summary.status
            }
            _ => return,
        };
        self.publish_update(key, session, status, Some(blocked));
    }

    fn publish_update(
        &self,
        key: &str,
        session: &SessionId,
        status: HarnessSessionStatus,
        blocked: Option<bool>,
    ) {
        let event = HarnessEvent::SessionUpdated {
            session_id: session.clone(),
            status,
            previous_session_id: None,
            supersedes: None,
            blocked,
        };
        self.publish_harness(key, event.clone());
        self.publish_registry(key.to_owned(), event);
    }
}
