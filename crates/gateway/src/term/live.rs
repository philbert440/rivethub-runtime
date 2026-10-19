use std::path::PathBuf;

use strum::IntoEnumIterator as _;
use tokio::sync::broadcast;
use wire::harness::{
    HarnessEvent, HarnessId, HarnessSessionStatus, HarnessSessionSummary, RosterCommand,
};

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
        let sender = self.harness_events.read().get(key).cloned();
        if let Some(sender) = sender
            && sender.send(event).is_err()
        {
            tracing::trace!("harness event has no live session socket");
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
        if let Some(summary) = self.summaries.write().get_mut(key) {
            summary.status = status;
            summary.updated_at = chrono::Utc::now();
        }
        let event = HarnessEvent::SessionUpdated {
            session_id: session.clone(),
            status,
            previous_session_id: None,
            supersedes: None,
            blocked: None,
        };
        self.publish_harness(key, event.clone());
        self.publish_registry(key.to_owned(), event);
    }
}
