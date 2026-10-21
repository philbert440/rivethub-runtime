use std::collections::BTreeMap;

use kernel::harness::{SessionEvent, ToolCallStatus};
use wire::harness::HarnessEvent;
use wire::session_id::SessionId;
use wire::stream::LlmUsage;

#[derive(Debug, Default)]
pub(crate) struct ToolNames {
    names: BTreeMap<String, String>,
    usage: Option<LlmUsage>,
}

impl ToolNames {
    pub(crate) fn harness_event(
        &mut self,
        session: &SessionId,
        event: &SessionEvent,
    ) -> Option<HarnessEvent> {
        let session_id = session.clone();
        Some(match event {
            SessionEvent::AgentMessageChunk { text } => HarnessEvent::AssistantDelta {
                session_id,
                text: text.clone(),
                turn_id: None,
            },
            SessionEvent::AgentThoughtChunk { text } => HarnessEvent::ReasoningDelta {
                session_id,
                text: text.clone(),
                turn_id: None,
            },
            SessionEvent::ToolCall(call) => {
                self.names
                    .insert(call.tool_call_id.clone(), call.name.clone());
                HarnessEvent::ToolUse {
                    session_id,
                    tool_call_id: call.tool_call_id.clone(),
                    name: call.name.clone(),
                    input: call.input.clone(),
                    turn_id: None,
                }
            }
            SessionEvent::ToolCallUpdate(update) => {
                let failed = match update.status {
                    ToolCallStatus::Running => return None,
                    ToolCallStatus::Completed => false,
                    ToolCallStatus::Failed => true,
                };
                HarnessEvent::ToolResult {
                    session_id,
                    name: self.names.remove(&update.tool_call_id)?,
                    tool_call_id: update.tool_call_id.clone(),
                    output: update.output.clone().unwrap_or_default(),
                    is_error: failed.then_some(true),
                    turn_id: None,
                }
            }
            SessionEvent::Permission(request) => HarnessEvent::ApprovalRequest {
                session_id,
                request_id: request.request_id.clone(),
                tool_call_id: request.tool_call_id.clone(),
                name: request.name.clone(),
                input: request.input.clone(),
                reason: request.reason.clone(),
                options: None,
            },
            SessionEvent::Usage { usage } => {
                self.usage = Some(usage.clone());
                return None;
            }
            SessionEvent::TurnEnded => HarnessEvent::TurnComplete {
                session_id,
                turn_id: None,
                stop_reason: None,
                usage: self.usage.take(),
            },
            SessionEvent::UserMessageChunk { .. }
            | SessionEvent::Plan(_)
            | SessionEvent::AvailableCommands(_)
            | SessionEvent::CurrentMode { .. }
            | SessionEvent::Exited { .. }
            | SessionEvent::TerminalBytes { .. }
            | SessionEvent::TerminalResize { .. } => return None,
        })
    }
}
