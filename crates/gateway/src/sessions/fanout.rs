use std::collections::BTreeMap;
use std::time::Duration;

use kernel::harness::{
    ContextUsage, ContextUsageSource, SessionEvent, ToolCallStatus, ToolCallUpdate,
};
use tokio::time::Instant;
use wire::harness::HarnessEvent;
use wire::session_id::SessionId;
use wire::stream::LlmUsage;

#[derive(Debug, Default)]
pub(crate) struct ToolNames {
    names: BTreeMap<String, String>,
    usage: Option<LlmUsage>,
    clock: ModelClock,
}

#[derive(Debug, Default)]
struct ModelClock {
    since: Option<Instant>,
    spent: Duration,
}

impl ModelClock {
    fn resume(&mut self) {
        self.since.get_or_insert_with(Instant::now);
    }

    fn pause(&mut self) {
        if let Some(since) = self.since.take() {
            self.spent += since.elapsed();
        }
    }

    fn stop(&mut self) -> Option<u64> {
        self.pause();
        let spent = std::mem::take(&mut self.spent);
        let ms = spent
            .as_secs()
            .saturating_mul(1000)
            .saturating_add(u64::from(spent.subsec_millis()));
        (ms > 0).then_some(ms)
    }

    fn track(&mut self, event: &SessionEvent) {
        match event {
            SessionEvent::UserMessageChunk { .. }
            | SessionEvent::AgentMessageChunk { .. }
            | SessionEvent::AgentThoughtChunk { .. } => self.resume(),
            SessionEvent::ToolCallUpdate(update) if update.status != ToolCallStatus::Running => {
                self.resume();
            }
            SessionEvent::ToolCall(_) | SessionEvent::Permission(_) => self.pause(),
            SessionEvent::ToolCallUpdate(_)
            | SessionEvent::Usage { .. }
            | SessionEvent::Context(_)
            | SessionEvent::Plan(_)
            | SessionEvent::AvailableCommands(_)
            | SessionEvent::CurrentMode { .. }
            | SessionEvent::TurnEnded
            | SessionEvent::Exited { .. }
            | SessionEvent::TerminalBytes { .. }
            | SessionEvent::TerminalResize { .. } => {}
        }
    }
}

fn context_event(session_id: SessionId, context: &ContextUsage) -> HarnessEvent {
    HarnessEvent::Context {
        session_id,
        used: context.used,
        window: context.window,
        source: match context.source {
            ContextUsageSource::Harness => wire::events::ContextUsageSource::Harness,
            ContextUsageSource::LastCall => wire::events::ContextUsageSource::LastCall,
        },
    }
}

impl ToolNames {
    pub(crate) fn harness_event(
        &mut self,
        session: &SessionId,
        event: &SessionEvent,
    ) -> Option<HarnessEvent> {
        let session_id = session.clone();
        self.clock.track(event);
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
                model_ms: self.clock.stop(),
            },
            SessionEvent::Context(context) => context_event(session_id, context),
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
    use std::time::Duration;

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

    #[tokio::test(start_paused = true)]
    async fn a_turn_reports_the_model_time_without_the_tool_time() {
        let session = SessionId::new(HarnessId::GrokBuild, "native");
        let mut tools = ToolNames::default();
        let started = SessionEvent::ToolCall(ToolCallInfo {
            tool_call_id: "call".to_owned(),
            name: "Bash".to_owned(),
            input: JsonObject::default(),
        });
        let steps = [
            (
                0,
                SessionEvent::UserMessageChunk {
                    text: "go".to_owned(),
                },
            ),
            (4_000, started),
            (60_000, update(ToolCallStatus::Completed, None)),
            (
                1_500,
                SessionEvent::AgentMessageChunk {
                    text: "done".to_owned(),
                },
            ),
            (500, SessionEvent::TurnEnded),
        ];
        let mut ended = None;
        for (after, event) in steps {
            tokio::time::advance(Duration::from_millis(after)).await;
            ended = tools.harness_event(&session, &event);
        }
        assert!(matches!(
            ended,
            Some(HarnessEvent::TurnComplete {
                model_ms: Some(6_000),
                ..
            })
        ));
    }
}
