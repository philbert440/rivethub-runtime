use agent_client_protocol::schema::v1 as acp;
use kernel::harness::Command;
use kernel::harness::PlanEntry;
use kernel::harness::PlanPriority;
use kernel::harness::SessionEvent;
use kernel::harness::ToolCallInfo;
use kernel::harness::ToolCallStatus;
use kernel::harness::ToolCallUpdate;
use wire::json::JsonObject;

use crate::error::AcpError;

pub(crate) fn session_event(update: acp::SessionUpdate) -> Result<Option<SessionEvent>, AcpError> {
    let event = match update {
        acp::SessionUpdate::UserMessageChunk(chunk) => Ok(SessionEvent::UserMessageChunk {
            text: chunk_text(&chunk.content)?,
        }),
        acp::SessionUpdate::AgentMessageChunk(chunk) => Ok(SessionEvent::AgentMessageChunk {
            text: chunk_text(&chunk.content)?,
        }),
        acp::SessionUpdate::AgentThoughtChunk(chunk) => Ok(SessionEvent::AgentThoughtChunk {
            text: chunk_text(&chunk.content)?,
        }),
        acp::SessionUpdate::ToolCall(call) => Ok(SessionEvent::ToolCall(ToolCallInfo {
            tool_call_id: call.tool_call_id.0.to_string(),
            name: call.title,
            input: object_input(call.raw_input)?,
        })),
        acp::SessionUpdate::ToolCallUpdate(update) => tool_update(&update),
        acp::SessionUpdate::Plan(plan) => plan_event(plan),
        acp::SessionUpdate::AvailableCommandsUpdate(update) => Ok(SessionEvent::AvailableCommands(
            update
                .available_commands
                .into_iter()
                .map(|command| Command {
                    name: command.name,
                    description: command.description,
                })
                .collect(),
        )),
        acp::SessionUpdate::CurrentModeUpdate(update) => Ok(SessionEvent::CurrentMode {
            mode: update.current_mode_id.to_string(),
        }),
        acp::SessionUpdate::ConfigOptionUpdate(_)
        | acp::SessionUpdate::SessionInfoUpdate(_)
        | acp::SessionUpdate::UsageUpdate(_)
        | acp::SessionUpdate::Notice(_)
        | acp::SessionUpdate::CompactionUpdate(_)
        | acp::SessionUpdate::CompactionSummaryChunk(_) => return Ok(None),
        _ => Err(AcpError::UnmappedUpdate),
    };
    event.map(Some)
}

pub(crate) fn permission_request(
    request_id: &str,
    request: &acp::RequestPermissionRequest,
) -> Result<kernel::harness::PermissionRequest, AcpError> {
    Ok(kernel::harness::PermissionRequest {
        request_id: request_id.to_owned(),
        tool_call_id: Some(request.tool_call.tool_call_id.0.to_string()),
        name: request
            .tool_call
            .fields
            .title
            .clone()
            .ok_or(AcpError::MissingPermissionTitle)?,
        input: object_input(request.tool_call.fields.raw_input.clone())?,
        reason: None,
    })
}

fn tool_update(update: &acp::ToolCallUpdate) -> Result<SessionEvent, AcpError> {
    let status = match update.fields.status {
        Some(status) => tool_status(status)?,
        None => ToolCallStatus::Running,
    };
    let output = content_text(update.fields.content.as_deref())?;
    Ok(SessionEvent::ToolCallUpdate(ToolCallUpdate {
        tool_call_id: update.tool_call_id.0.to_string(),
        status,
        output,
    }))
}

fn plan_event(plan: acp::Plan) -> Result<SessionEvent, AcpError> {
    let mut entries = Vec::with_capacity(plan.entries.len());
    for entry in plan.entries {
        entries.push(PlanEntry {
            content: entry.content,
            priority: plan_priority(&entry.priority)?,
        });
    }
    Ok(SessionEvent::Plan(entries))
}

fn object_input(raw: Option<impl serde::Serialize>) -> Result<JsonObject, AcpError> {
    let Some(raw) = raw else {
        return Ok(JsonObject::default());
    };
    let value = serde_json::to_value(raw).map_err(|source| AcpError::ToolInput { source })?;
    serde_json::from_value(value).map_err(|source| AcpError::ToolInput { source })
}

fn chunk_text(content: &acp::ContentBlock) -> Result<String, AcpError> {
    match content {
        acp::ContentBlock::Text(text) => Ok(text.text.clone()),
        acp::ContentBlock::ResourceLink(link) => Ok(link.uri.clone()),
        acp::ContentBlock::Resource(resource) => match &resource.resource {
            acp::EmbeddedResourceResource::TextResourceContents(text) => Ok(text.text.clone()),
            acp::EmbeddedResourceResource::BlobResourceContents(blob) => Ok(blob.uri.clone()),
            _ => Err(AcpError::NonTextContent),
        },
        acp::ContentBlock::Image(_) | acp::ContentBlock::Audio(_) | _ => {
            Err(AcpError::NonTextContent)
        }
    }
}

fn content_text(contents: Option<&[acp::ToolCallContent]>) -> Result<Option<String>, AcpError> {
    let Some(contents) = contents else {
        return Ok(None);
    };
    let mut texts = Vec::with_capacity(contents.len());
    for content in contents {
        match content {
            acp::ToolCallContent::Content(content) => texts.push(chunk_text(&content.content)?),
            acp::ToolCallContent::Diff(diff) => texts.push(diff.new_text.clone()),
            acp::ToolCallContent::Terminal(_) | _ => return Err(AcpError::NonTextContent),
        }
    }
    Ok(Some(texts.join("\n")))
}

fn tool_status(status: acp::ToolCallStatus) -> Result<ToolCallStatus, AcpError> {
    match status {
        acp::ToolCallStatus::Completed => Ok(ToolCallStatus::Completed),
        acp::ToolCallStatus::Failed => Ok(ToolCallStatus::Failed),
        acp::ToolCallStatus::Pending | acp::ToolCallStatus::InProgress => {
            Ok(ToolCallStatus::Running)
        }
        _ => Err(AcpError::UnmappedUpdate),
    }
}

fn plan_priority(priority: &acp::PlanEntryPriority) -> Result<PlanPriority, AcpError> {
    match priority {
        acp::PlanEntryPriority::High => Ok(PlanPriority::High),
        acp::PlanEntryPriority::Medium => Ok(PlanPriority::Medium),
        acp::PlanEntryPriority::Low => Ok(PlanPriority::Low),
        _ => Err(AcpError::UnmappedUpdate),
    }
}
