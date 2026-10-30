use kernel::harness::SessionEvent;
use wire::events::{AgentEvent, AgentEventKind, ProtocolVersion};
use wire::harness::HarnessSessionStatus;

use super::ChatActor;
use crate::error::ApiError;

impl ChatActor {
    fn fan_out(&mut self, event: &SessionEvent) {
        if let SessionEvent::ToolCallUpdate(update) = event {
            self.service.tool_updated(&self.key, update);
        }
        if let Some(harness_event) = self.tools.harness_event(&self.session, event) {
            self.service.publish_harness(&self.key, harness_event);
        }
        if matches!(event, SessionEvent::Permission(_)) {
            self.service.set_blocked(&self.key, &self.session, true);
            return;
        }
        let status = if matches!(event, SessionEvent::TurnEnded) {
            HarnessSessionStatus::Idle
        } else if matches!(event, SessionEvent::Exited { .. }) {
            HarnessSessionStatus::Ended
        } else {
            return;
        };
        self.service
            .set_live_status(&self.key, &self.session, status);
    }

    pub(super) fn publish(&mut self, event: SessionEvent) -> Result<(), ApiError> {
        self.fan_out(&event);
        let ts = Some(chrono::Utc::now());
        let kind = match event {
            SessionEvent::UserMessageChunk { text } => AgentEventKind::MessageUser { text, ts },
            SessionEvent::AgentMessageChunk { text } => AgentEventKind::MessageAgent {
                text,
                model: None,
                duration_ms: None,
                usage: None,
                ts,
            },
            SessionEvent::AgentThoughtChunk { text } => AgentEventKind::ThinkingDelta { text, ts },
            SessionEvent::ToolCall(call) => AgentEventKind::ToolStart {
                tool: call.name,
                args: Some(call.input),
                ts,
            },
            SessionEvent::Plan(plan) => AgentEventKind::TaskPlan {
                tasks: plan.into_iter().map(|entry| entry.content).collect(),
                ts,
            },
            SessionEvent::TurnEnded => AgentEventKind::TurnEnd { ts },
            SessionEvent::Exited { .. } => AgentEventKind::SessionEnd { ts },
            SessionEvent::Permission(request) => AgentEventKind::ToolStart {
                tool: request.name,
                args: Some(request.input),
                ts,
            },
            SessionEvent::ToolCallUpdate(_)
            | SessionEvent::Usage { .. }
            | SessionEvent::Context(_)
            | SessionEvent::AvailableCommands(_)
            | SessionEvent::CurrentMode { .. }
            | SessionEvent::TerminalBytes { .. }
            | SessionEvent::TerminalResize { .. } => {
                tracing::trace!("harness metadata has no v1 session projection");
                return Ok(());
            }
        };
        self.service.registry.ingest(
            &self.user,
            vec![AgentEvent {
                v: ProtocolVersion::V1,
                session: self.key.clone(),
                kind,
                extra: wire::Extra::default(),
            }],
        )
    }
}
