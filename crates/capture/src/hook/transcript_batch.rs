use chrono::DateTime;
use chrono::Utc;
use wire::JsonObject;
use wire::capture::CaptureBatchRequest;
use wire::capture::CaptureMessage;
use wire::message::Role;

use super::FALLBACK_TITLE;
use super::HookError;
use super::base_batch;
use super::is_prompt;
use crate::event_id::OccurrenceKind;
use crate::event_id::occurrence_digest;
use crate::event_id::occurrence_event_id;
use crate::payload::HookEvent;
use crate::payload::HookPayload;
use crate::records::SOURCE;
use crate::records::TranscriptMetadata;
use crate::transcript::Line;
use crate::transcript::LineKind;
use crate::transcript::ToolOutcome;
use crate::transcript::ToolUse;

struct Source<'a> {
    session_key: &'a str,
    path: Option<String>,
    transcript: &'a [Line],
}

pub(super) fn build(
    payload: &HookPayload,
    session_key: &str,
    transcript: &[Line],
    event: HookEvent,
) -> Result<CaptureBatchRequest, HookError> {
    let source = Source {
        session_key,
        path: payload
            .transcript_path
            .as_ref()
            .map(|path| path.display().to_string()),
        transcript,
    };
    let mut batch = base_batch(session_key);
    batch.finalize = (event == HookEvent::SessionEnd).then_some(true);
    assistant_turns(&mut batch, &source)?;
    recovered_tools(&mut batch, &source)?;
    recovered_prompts(&mut batch, &source)?;
    batch.title = Some(
        transcript
            .iter()
            .filter(|line| is_prompt(line))
            .find_map(Line::text)
            .unwrap_or_else(|| FALLBACK_TITLE.to_owned()),
    );
    Ok(batch)
}

impl Source<'_> {
    fn metadata(
        &self,
        position: usize,
        uuid: Option<&str>,
        sidechain: Option<bool>,
        hook_event: Option<&str>,
        recovered: Option<bool>,
    ) -> Result<JsonObject, HookError> {
        Ok(JsonObject::from_serialize(&TranscriptMetadata {
            source: SOURCE,
            session_jsonl_path: self.path.clone(),
            session_jsonl_line: Some(position),
            uuid,
            sidechain,
            hook_event,
            recovered,
        })?)
    }
}

fn assistant_digest(line: &Line) -> Option<(String, String)> {
    if line.r#type != LineKind::Assistant {
        return None;
    }
    let content = line.text()?;
    let digest = occurrence_digest(OccurrenceKind::Assistant, &content, "", None);
    Some((content, digest))
}

fn assistant_turns(batch: &mut CaptureBatchRequest, source: &Source<'_>) -> Result<(), HookError> {
    let mut seen: Vec<String> = Vec::new();
    for (position, line) in source.transcript.iter().enumerate() {
        let Some((content, digest)) = assistant_digest(line) else {
            continue;
        };
        let occurrence = seen.iter().filter(|earlier| **earlier == digest).count();
        seen.push(digest.clone());
        if content.is_empty() {
            continue;
        }
        let event_id = match &line.uuid {
            Some(uuid) => format!("{}:{uuid}", source.session_key),
            None => occurrence_event_id(source.session_key, &digest, occurrence),
        };
        let sidechain = line.is_sidechain.unwrap_or(false);
        batch.messages.push(CaptureMessage {
            event_id,
            role: Role::Assistant,
            content,
            tool_name: None,
            tool_args: None,
            tool_result: None,
            metadata: Some(source.metadata(
                position,
                line.uuid.as_deref(),
                Some(sidechain),
                None,
                None,
            )?),
            created_at: instant(line.timestamp.as_deref())?,
        });
    }
    Ok(())
}

struct Issuer<'a> {
    line: &'a Line,
    tool_use: ToolUse<'a>,
}

fn tool_use_issuer<'a>(transcript: &'a [Line], outcome: &ToolOutcome) -> Option<Issuer<'a>> {
    let tool_use_id = outcome.tool_use_id.as_deref()?;
    transcript.iter().find_map(|line| {
        line.tool_uses()
            .into_iter()
            .find(|tool_use| tool_use.id == tool_use_id)
            .map(|tool_use| Issuer { line, tool_use })
    })
}

fn recovered_tools(batch: &mut CaptureBatchRequest, source: &Source<'_>) -> Result<(), HookError> {
    let user_lines = source
        .transcript
        .iter()
        .enumerate()
        .filter(|(_, line)| line.r#type == LineKind::User);
    for (position, line) in user_lines {
        for outcome in line.tool_results() {
            let Some(Issuer {
                line: issuer,
                tool_use,
            }) = tool_use_issuer(source.transcript, &outcome)
            else {
                continue;
            };
            let content = format!("[tool call] {}", tool_use.name);
            let event_id = match outcome.tool_use_id.as_deref() {
                Some(id) if !id.is_empty() => format!("{}:tool:{id}", source.session_key),
                Some(_) | None => {
                    let digest = occurrence_digest(
                        OccurrenceKind::Tool,
                        &content,
                        tool_use.name,
                        Some(tool_use.input),
                    );
                    occurrence_event_id(source.session_key, &digest, 0)
                }
            };
            batch.messages.push(CaptureMessage {
                event_id,
                role: Role::Tool,
                content,
                tool_name: Some(tool_use.name.to_owned()),
                tool_args: Some(tool_use.input.arguments().clone()),
                tool_result: Some(outcome.text),
                metadata: Some(source.metadata(
                    position,
                    issuer.uuid.as_deref(),
                    None,
                    Some("PostToolUse"),
                    Some(true),
                )?),
                created_at: instant(issuer.timestamp.as_deref())?,
            });
        }
    }
    Ok(())
}

fn instant(raw: Option<&str>) -> Result<Option<DateTime<Utc>>, HookError> {
    raw.map(|text| DateTime::parse_from_rfc3339(text).map(|parsed| parsed.with_timezone(&Utc)))
        .transpose()
        .map_err(HookError::Timestamp)
}

fn recovered_prompts(
    batch: &mut CaptureBatchRequest,
    source: &Source<'_>,
) -> Result<(), HookError> {
    for (position, line) in source.transcript.iter().enumerate() {
        if !is_prompt(line) {
            continue;
        }
        let Some(content) = line.text() else {
            continue;
        };
        batch.messages.push(CaptureMessage {
            event_id: format!("{}:u{}", source.session_key, position + 1),
            role: Role::User,
            content,
            tool_name: None,
            tool_args: None,
            tool_result: None,
            metadata: Some(source.metadata(
                position,
                line.uuid.as_deref(),
                None,
                None,
                Some(true),
            )?),
            created_at: instant(line.timestamp.as_deref())?,
        });
    }
    Ok(())
}
