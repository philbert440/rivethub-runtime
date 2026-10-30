use std::collections::BTreeMap;

use kernel::harness::{SessionEvent, ToolCallStatus, ToolCallUpdate};
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
            SessionEvent::ToolCallUpdate(update) => return self.tool_updated(session_id, update),
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
            SessionEvent::Plan(entries) => HarnessEvent::Plan {
                session_id,
                entries: entries.iter().map(plan_entry).collect(),
            },
            SessionEvent::UserMessageChunk { .. }
            | SessionEvent::AvailableCommands(_)
            | SessionEvent::CurrentMode { .. }
            | SessionEvent::Exited { .. }
            | SessionEvent::TerminalBytes { .. }
            | SessionEvent::TerminalResize { .. } => return None,
        })
    }

    fn tool_updated(
        &mut self,
        session_id: SessionId,
        update: &ToolCallUpdate,
    ) -> Option<HarnessEvent> {
        let failed = match update.status {
            ToolCallStatus::Running => {
                if !self.names.contains_key(&update.tool_call_id) {
                    return None;
                }
                let input = update
                    .input
                    .clone()
                    .filter(|input| !input.as_map().is_empty())?;
                return Some(HarnessEvent::ToolUpdate {
                    session_id,
                    tool_call_id: update.tool_call_id.clone(),
                    input,
                    turn_id: None,
                });
            }
            ToolCallStatus::Completed => false,
            ToolCallStatus::Failed => true,
        };
        Some(HarnessEvent::ToolResult {
            session_id,
            name: self.names.remove(&update.tool_call_id)?,
            tool_call_id: update.tool_call_id.clone(),
            output: update.output.clone().unwrap_or_default(),
            is_error: failed.then_some(true),
            turn_id: None,
        })
    }
}

fn plan_entry(entry: &kernel::harness::PlanEntry) -> wire::harness::PlanEntry {
    wire::harness::PlanEntry {
        content: entry.content.clone(),
        status: entry.status,
    }
}

#[cfg(test)]
mod tests {
    use kernel::harness::{SessionEvent, ToolCallInfo, ToolCallStatus, ToolCallUpdate};
    use wire::harness::{HarnessEvent, HarnessId};
    use wire::json::JsonObject;
    use wire::session_id::SessionId;

    use super::ToolNames;

    fn update(status: ToolCallStatus, input: Option<JsonObject>) -> SessionEvent {
        SessionEvent::ToolCallUpdate(ToolCallUpdate {
            tool_call_id: "call".to_owned(),
            status,
            output: None,
            title: None,
            input,
        })
    }

    #[test]
    fn a_running_tool_republishes_the_input_a_later_update_names() {
        let session = SessionId::new(HarnessId::ClaudeCode, "native");
        let mut tools = ToolNames::default();
        let started = SessionEvent::ToolCall(ToolCallInfo {
            tool_call_id: "call".to_owned(),
            name: "Bash".to_owned(),
            input: JsonObject::default(),
        });
        assert!(matches!(
            tools.harness_event(&session, &started),
            Some(HarnessEvent::ToolUse { .. })
        ));
        let mut input = JsonObject::default();
        input.insert("command", "cargo test").expect("input");
        assert_eq!(
            tools.harness_event(&session, &update(ToolCallStatus::Running, None)),
            None
        );
        let Some(HarnessEvent::ToolUpdate { input: named, .. }) = tools.harness_event(
            &session,
            &update(ToolCallStatus::Running, Some(input.clone())),
        ) else {
            panic!("tool update")
        };
        assert_eq!(named, input);
        let finished = tools.harness_event(&session, &update(ToolCallStatus::Completed, None));
        assert!(matches!(finished, Some(HarnessEvent::ToolResult { name, .. }) if name == "Bash"));
        assert_eq!(
            tools.harness_event(&session, &update(ToolCallStatus::Running, Some(input))),
            None
        );
    }
}
