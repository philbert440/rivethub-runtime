use agent_client_protocol as acp;
use kernel::harness::Command;
use kernel::harness::PlanEntry;
use kernel::harness::PlanPriority;
use kernel::harness::SessionEvent;
use kernel::harness::ToolCallInfo;
use kernel::harness::ToolCallStatus;
use kernel::harness::ToolCallUpdate;
use wire::json::JsonObject;

use crate::error::AcpError;

pub(crate) fn session_event(update: acp::SessionUpdate) -> Result<SessionEvent, AcpError> {
    match update {
        acp::SessionUpdate::UserMessageChunk { content } => Ok(SessionEvent::UserMessageChunk {
            text: chunk_text(&content),
        }),
        acp::SessionUpdate::AgentMessageChunk { content } => Ok(SessionEvent::AgentMessageChunk {
            text: chunk_text(&content),
        }),
        acp::SessionUpdate::AgentThoughtChunk { content } => Ok(SessionEvent::AgentThoughtChunk {
            text: chunk_text(&content),
        }),
        acp::SessionUpdate::ToolCall(call) => {
            let input = match call.raw_input {
                Some(raw) if raw.is_null() => JsonObject::default(),
                Some(raw) => {
                    serde_json::from_value(raw).map_err(|source| AcpError::ToolInput { source })?
                }
                None => JsonObject::default(),
            };
            Ok(SessionEvent::ToolCall(ToolCallInfo {
                tool_call_id: call.id.0.to_string(),
                name: call.title,
                input,
            }))
        }
        acp::SessionUpdate::ToolCallUpdate(update) => {
            let output = content_text(update.fields.content.as_deref());
            Ok(SessionEvent::ToolCallUpdate(ToolCallUpdate {
                tool_call_id: update.id.0.to_string(),
                status: tool_status(update.fields.status),
                output: if output.is_empty() {
                    None
                } else {
                    Some(output)
                },
            }))
        }
        acp::SessionUpdate::Plan(plan) => Ok(SessionEvent::Plan(
            plan.entries
                .into_iter()
                .map(|entry| PlanEntry {
                    content: entry.content,
                    priority: plan_priority(&entry.priority),
                })
                .collect(),
        )),
        acp::SessionUpdate::AvailableCommandsUpdate { available_commands } => {
            Ok(SessionEvent::AvailableCommands(
                available_commands
                    .into_iter()
                    .map(|command| Command {
                        name: command.name,
                        description: command.description,
                    })
                    .collect(),
            ))
        }
        acp::SessionUpdate::CurrentModeUpdate { current_mode_id } => {
            Ok(SessionEvent::CurrentMode {
                mode: current_mode_id.to_string(),
            })
        }
    }
}

pub(crate) fn permission_request(
    request_id: &str,
    request: &acp::RequestPermissionRequest,
) -> Result<kernel::harness::PermissionRequest, AcpError> {
    let input = match &request.tool_call.fields.raw_input {
        Some(raw) if raw.is_null() => JsonObject::default(),
        Some(raw) => {
            serde_json::from_value(raw.clone()).map_err(|source| AcpError::ToolInput { source })?
        }
        None => JsonObject::default(),
    };
    Ok(kernel::harness::PermissionRequest {
        request_id: request_id.to_owned(),
        tool_call_id: Some(request.tool_call.id.0.to_string()),
        name: match &request.tool_call.fields.title {
            Some(title) => title.clone(),
            None => String::new(),
        },
        input,
        reason: None,
    })
}

fn chunk_text(content: &acp::ContentBlock) -> String {
    match content {
        acp::ContentBlock::Text(text) => text.text.clone(),
        acp::ContentBlock::ResourceLink(link) => link.uri.clone(),
        acp::ContentBlock::Resource(resource) => match &resource.resource {
            acp::EmbeddedResourceResource::TextResourceContents(text) => text.text.clone(),
            acp::EmbeddedResourceResource::BlobResourceContents(blob) => blob.uri.clone(),
        },
        acp::ContentBlock::Image(_) | acp::ContentBlock::Audio(_) => String::new(),
    }
}

fn content_text(contents: Option<&[acp::ToolCallContent]>) -> String {
    let Some(contents) = contents else {
        return String::new();
    };
    let texts: Vec<String> = contents
        .iter()
        .filter_map(|content| match content {
            acp::ToolCallContent::Content { content } => match content {
                acp::ContentBlock::Text(text) => Some(text.text.clone()),
                acp::ContentBlock::ResourceLink(link) => Some(link.uri.clone()),
                acp::ContentBlock::Resource(resource) => match &resource.resource {
                    acp::EmbeddedResourceResource::TextResourceContents(text) => {
                        Some(text.text.clone())
                    }
                    acp::EmbeddedResourceResource::BlobResourceContents(blob) => {
                        Some(blob.uri.clone())
                    }
                },
                acp::ContentBlock::Image(_) | acp::ContentBlock::Audio(_) => None,
            },
            acp::ToolCallContent::Diff { diff } => Some(diff.new_text.clone()),
            acp::ToolCallContent::Terminal { .. } => None,
        })
        .collect();
    texts.join("\n")
}

fn tool_status(status: Option<acp::ToolCallStatus>) -> ToolCallStatus {
    match status {
        Some(acp::ToolCallStatus::Completed) => ToolCallStatus::Completed,
        Some(acp::ToolCallStatus::Failed) => ToolCallStatus::Failed,
        Some(acp::ToolCallStatus::Pending | acp::ToolCallStatus::InProgress) | None => {
            ToolCallStatus::Running
        }
    }
}

fn plan_priority(priority: &acp::PlanEntryPriority) -> PlanPriority {
    match priority {
        acp::PlanEntryPriority::High => PlanPriority::High,
        acp::PlanEntryPriority::Medium => PlanPriority::Medium,
        acp::PlanEntryPriority::Low => PlanPriority::Low,
    }
}
