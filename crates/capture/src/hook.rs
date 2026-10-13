use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;
use serde_json::value::RawValue;
use thiserror::Error;
use wire::JsonObject;
use wire::capture::CaptureBatchRequest;
use wire::capture::CaptureMessage;
use wire::message::Role;
use wire::session_id::SessionId;

use crate::event_id::OccurrenceKind;
use crate::event_id::arguments_spelling;
use crate::event_id::occurrence_digest;
use crate::event_id::occurrence_event_id;
use crate::json;
use crate::json::JsJson;
use crate::json::JsonError;
use crate::payload::HookEvent;
use crate::payload::HookPayload;
use crate::records::HookEventMetadata;
use crate::records::PromptSettings;
use crate::records::SOURCE;
use crate::records::TranscriptMetadata;
use crate::records::TranscriptSettings;
use crate::transcript::Line;
use crate::transcript::LineKind;
use crate::transcript::ToolOutcome;
use crate::transcript::parse_lines;
use crate::transcript::read_capped;
use crate::transcript::report_skipped;
use crate::writer::CaptureTransport;
use crate::writer::CaptureWriter;
use crate::writer::Totals;

pub const AGENT: &str = "rivet-claude";
pub const CHANNEL: &str = "claude-code";
const FALLBACK_TITLE: &str = "Claude Code session";
const LOG_FILE: &str = "claude-capture.log";
const LOG_DIR: &str = ".rivetos";

#[derive(Debug, Error)]
pub enum HookError {
    #[error("hook payload is missing {0}")]
    Missing(&'static str),
    #[error(transparent)]
    Json(#[from] JsonError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRun {
    pub exit: i64,
    pub stderr: String,
}

impl HookRun {
    #[must_use]
    pub fn done() -> Self {
        Self {
            exit: 0,
            stderr: String::new(),
        }
    }

    fn failed(message: String) -> Self {
        Self {
            exit: 0,
            stderr: message,
        }
    }
}

#[derive(Debug)]
pub struct ClaudeHook<T: CaptureTransport> {
    writer: CaptureWriter<T>,
    state_dir: PathBuf,
}

impl<T: CaptureTransport> ClaudeHook<T> {
    #[must_use]
    pub fn new(writer: CaptureWriter<T>, state_dir: PathBuf) -> Self {
        Self { writer, state_dir }
    }

    pub async fn run(&self, stdin: &str, now: DateTime<Utc>, ingest_key: &str) -> HookRun {
        let payload = match serde_json::from_str::<HookPayload>(stdin) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::warn!("hook payload was not valid: {error}");
                return HookRun::done();
            }
        };
        let Some(event) = payload.hook_event_name else {
            return HookRun::done();
        };
        if event == HookEvent::Other {
            return HookRun::done();
        }
        let Some(session) = payload.session_id.clone() else {
            return HookRun::done();
        };
        let transcript = load_transcript(&payload);
        let batch = match event {
            HookEvent::UserPromptSubmit => {
                user_prompt_batch(&payload, session.as_str(), &transcript, ingest_key)
            }
            HookEvent::PostToolUse => {
                tool_batch(&payload, session.as_str(), &transcript, ingest_key)
            }
            HookEvent::Stop | HookEvent::SubagentStop | HookEvent::SessionEnd => {
                transcript_batch(&payload, session.as_str(), &transcript, event)
            }
            HookEvent::Other => return HookRun::done(),
        };
        let mut batch = match batch {
            Ok(batch) => batch,
            Err(error) => return HookRun::failed(error.to_string()),
        };
        match settings_for(&payload, event, now) {
            Ok(settings) => batch.settings = Some(settings),
            Err(error) => return HookRun::failed(error.to_string()),
        }
        match self.writer.write(&batch, now).await {
            Ok(totals) => {
                self.best_effort_log(&log_line(event, session.as_str(), &totals, now));
                HookRun::done()
            }
            Err(error) => HookRun::failed(error.to_string()),
        }
    }

    fn best_effort_log(&self, line: &str) {
        let dir = self.state_dir.join(LOG_DIR);
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let target = dir.join(LOG_FILE);
        if !target.exists() {
            drop(std::fs::write(&target, b""));
            set_log_mode(&target);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(&target) {
            drop(file.write_all(format!("{line}\n").as_bytes()));
        }
    }
}

fn load_transcript(payload: &HookPayload) -> Vec<Line> {
    let Some(path) = &payload.transcript_path else {
        return Vec::new();
    };
    match read_capped(path) {
        Ok(text) => {
            let parsed = parse_lines(&text);
            report_skipped(&parsed.skipped);
            parsed.lines
        }
        Err(error) => {
            tracing::warn!("transcript {} was unread: {error}", path.display());
            Vec::new()
        }
    }
}

fn set_log_mode(target: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    if let Ok(metadata) = std::fs::metadata(target) {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o644);
        drop(std::fs::set_permissions(target, permissions));
    }
}

fn iso_at(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn native_of(session: &SessionId) -> Result<&str, HookError> {
    crate::transcript::native_id(session).ok_or(HookError::Missing("claude-code session id"))
}

fn base_batch(session_key: &str) -> CaptureBatchRequest {
    CaptureBatchRequest {
        session_key: session_key.to_owned(),
        agent: AGENT.to_owned(),
        channel: Some(CHANNEL.to_owned()),
        title: None,
        settings: None,
        task_id: None,
        finalize: None,
        created_at: None,
        updated_at: None,
        messages: Vec::new(),
    }
}

fn settings_for(
    payload: &HookPayload,
    event: HookEvent,
    now: DateTime<Utc>,
) -> Result<JsonObject, HookError> {
    let Some(session) = &payload.session_id else {
        return Err(HookError::Missing("session_id"));
    };
    let native = native_of(session)?.to_owned();
    let cwd = payload.cwd.as_ref().map(|path| path.display().to_string());
    if event.reads_transcript() {
        Ok(json::encode(&TranscriptSettings {
            source: SOURCE,
            file: payload
                .transcript_path
                .as_ref()
                .map(|path| path.display().to_string()),
            session_id: native,
            pr_url: None,
            cwd,
            last_event: event,
            last_ingest_at: iso_at(now),
        })?)
    } else {
        Ok(json::encode(&PromptSettings {
            source: SOURCE,
            session_id: native,
            cwd,
            last_event: event,
            last_ingest_at: iso_at(now),
        })?)
    }
}

fn event_metadata<'a>(event: &'a str, ingest_key: &'a str) -> Result<JsonObject, HookError> {
    Ok(json::encode(&HookEventMetadata {
        source: SOURCE,
        hook_event: event,
        ingest_key,
    })?)
}

fn line_metadata<'a>(
    path: Option<String>,
    position: usize,
    uuid: Option<&'a str>,
    sidechain: Option<bool>,
    hook_event: Option<&'a str>,
    recovered: Option<bool>,
) -> Result<JsonObject, HookError> {
    Ok(json::encode(&TranscriptMetadata {
        source: SOURCE,
        session_jsonl_path: path,
        session_jsonl_line: Some(position),
        uuid,
        sidechain,
        hook_event,
        recovered,
    })?)
}

fn user_prompt_batch(
    payload: &HookPayload,
    session_key: &str,
    transcript: &[Line],
    ingest_key: &str,
) -> Result<CaptureBatchRequest, HookError> {
    let mut batch = base_batch(session_key);
    let prompt = payload.prompt.clone();
    batch.title = Some(match prompt.as_deref() {
        Some(text) if !text.is_empty() => text.to_owned(),
        Some(_) | None => FALLBACK_TITLE.to_owned(),
    });
    let prompt_text = prompt.unwrap_or_default();
    let suffix = user_ordinal(transcript, &prompt_text);
    let message = CaptureMessage {
        event_id: format!("{session_key}:u{suffix}"),
        role: Role::User,
        content: prompt_text,
        tool_name: None,
        tool_args: None,
        tool_result: None,
        metadata: Some(event_metadata("UserPromptSubmit", ingest_key)?),
        created_at: None,
    };
    batch.messages.push(message);
    Ok(batch)
}

fn user_ordinal(transcript: &[Line], prompt: &str) -> usize {
    let matches: Vec<usize> = transcript
        .iter()
        .enumerate()
        .filter(|(_, line)| {
            line.r#type == LineKind::User
                && line.tool_result_text().is_none()
                && line.text().as_deref() == Some(prompt)
        })
        .map(|(position, _)| position)
        .collect();
    match matches.last() {
        Some(position) => position + 1,
        None => {
            transcript
                .iter()
                .filter(|line| line.r#type == LineKind::User && line.tool_result_text().is_none())
                .count()
                + 1
        }
    }
}

fn tool_batch(
    payload: &HookPayload,
    session_key: &str,
    transcript: &[Line],
    ingest_key: &str,
) -> Result<CaptureBatchRequest, HookError> {
    let mut batch = base_batch(session_key);
    batch.title = Some(FALLBACK_TITLE.to_owned());
    let Some(tool_name) = &payload.tool_name else {
        return Err(HookError::Missing("tool_name"));
    };
    let Some(tool_input) = &payload.tool_input else {
        return Err(HookError::Missing("tool_input"));
    };
    let parsed = arguments(tool_input)?;
    let content = format!("[tool call] {tool_name}");
    let event_id = if let Some(id) = &payload.tool_use_id {
        format!("{session_key}:tool:{id}")
    } else {
        let digest = occurrence_digest(
            OccurrenceKind::Tool,
            &content,
            tool_name,
            Some(&parsed.value),
        );
        let occurrence = prior_occurrences(transcript, tool_name, &parsed.spelling);
        occurrence_event_id(session_key, &digest, occurrence)
    };
    let message = CaptureMessage {
        event_id,
        role: Role::Tool,
        content,
        tool_name: Some(tool_name.clone()),
        tool_args: Some(parsed.object),
        tool_result: tool_result_text(payload)?,
        metadata: Some(event_metadata("PostToolUse", ingest_key)?),
        created_at: None,
    };
    batch.messages.push(message);
    Ok(batch)
}

struct Arguments {
    value: JsJson,
    spelling: String,
    object: JsonObject,
}

impl Arguments {
    fn of(value: JsJson) -> Result<Self, HookError> {
        Ok(Self {
            spelling: arguments_spelling(&value),
            object: json::to_object(&value)?,
            value,
        })
    }
}

fn arguments(raw: &RawValue) -> Result<Arguments, HookError> {
    Arguments::of(json::parse(raw.get())?)
}

fn transcript_arguments(input: &JsJson) -> Result<Arguments, HookError> {
    Arguments::of(input.clone())
}

fn prior_occurrences(transcript: &[Line], tool_name: &str, args_spelling: &str) -> usize {
    transcript
        .iter()
        .flat_map(Line::tool_uses)
        .filter(|use_| use_.name == tool_name && arguments_spelling(use_.input) == args_spelling)
        .count()
}

fn tool_result_text(payload: &HookPayload) -> Result<Option<String>, HookError> {
    if let Some(result) = &payload.tool_result {
        return Ok(Some(result.clone()));
    }
    let Some(raw) = &payload.tool_response else {
        return Ok(None);
    };
    match json::parse(raw.get())? {
        JsJson::String(text) => Ok(Some(text)),
        value @ (JsJson::Nil
        | JsJson::Bool(_)
        | JsJson::Number(_)
        | JsJson::Array(_)
        | JsJson::Object(_)) => Ok(Some(json::spelling(&value))),
    }
}

fn transcript_batch(
    payload: &HookPayload,
    session_key: &str,
    transcript: &[Line],
    event: HookEvent,
) -> Result<CaptureBatchRequest, HookError> {
    let mut batch = base_batch(session_key);
    batch.finalize = (event == HookEvent::SessionEnd).then_some(true);
    let path = payload
        .transcript_path
        .as_ref()
        .map(|path| path.display().to_string());
    assistant_turns(&mut batch, transcript, session_key, path.as_deref())?;
    recovered_tools(&mut batch, transcript, session_key, path.as_deref())?;
    recovered_prompts(&mut batch, transcript, session_key, path.as_deref())?;
    batch.title = Some(
        transcript
            .iter()
            .filter(|line| line.r#type == LineKind::User && line.tool_result_text().is_none())
            .find_map(Line::text)
            .unwrap_or_else(|| FALLBACK_TITLE.to_owned()),
    );
    Ok(batch)
}

fn assistant_turns(
    batch: &mut CaptureBatchRequest,
    transcript: &[Line],
    session_key: &str,
    path: Option<&str>,
) -> Result<(), HookError> {
    for (position, line) in transcript.iter().enumerate() {
        if line.r#type != LineKind::Assistant {
            continue;
        }
        let Some(content) = line.text() else {
            continue;
        };
        if content.is_empty() {
            continue;
        }
        let digest = occurrence_digest(OccurrenceKind::Assistant, &content, "", None);
        #[expect(
            clippy::indexing_slicing,
            reason = "position comes from enumerate over this same transcript"
        )]
        let earlier = &transcript[..position];
        let occurrence = earlier
            .iter()
            .filter(|line| {
                line.r#type == LineKind::Assistant
                    && line.text().is_some_and(|text| {
                        occurrence_digest(OccurrenceKind::Assistant, &text, "", None) == digest
                    })
            })
            .count();
        let event_id = match &line.uuid {
            Some(uuid) => format!("{session_key}:{uuid}"),
            None => occurrence_event_id(session_key, &digest, occurrence),
        };
        let metadata = line_metadata(
            path.map(str::to_owned),
            position,
            line.uuid.as_deref(),
            Some(sidechain_flag(line)),
            None,
            None,
        )?;
        batch.messages.push(CaptureMessage {
            event_id,
            role: Role::Assistant,
            content,
            tool_name: None,
            tool_args: None,
            tool_result: None,
            metadata: Some(metadata),
            created_at: line.timestamp.clone(),
        });
    }
    Ok(())
}

fn recovered_tools(
    batch: &mut CaptureBatchRequest,
    transcript: &[Line],
    session_key: &str,
    path: Option<&str>,
) -> Result<(), HookError> {
    for (position, line) in transcript.iter().enumerate() {
        if line.r#type != LineKind::User {
            continue;
        }
        for outcome in line.tool_results() {
            let Some(issuer) = tool_use_issuer(transcript, &outcome) else {
                continue;
            };
            let content = format!("[tool call] {}", issuer.name);
            let parsed = transcript_arguments(issuer.input)?;
            let event_id = match outcome.tool_use_id.as_deref() {
                Some(id) if !id.is_empty() => format!("{session_key}:tool:{id}"),
                _ => {
                    let digest = occurrence_digest(
                        OccurrenceKind::Tool,
                        &content,
                        issuer.name,
                        Some(&parsed.value),
                    );
                    occurrence_event_id(session_key, &digest, 0)
                }
            };
            let metadata = line_metadata(
                path.map(str::to_owned),
                position,
                issuer.line.uuid.as_deref(),
                None,
                Some("PostToolUse"),
                Some(true),
            )?;
            batch.messages.push(CaptureMessage {
                event_id,
                role: Role::Tool,
                content,
                tool_name: Some(issuer.name.to_owned()),
                tool_args: Some(parsed.object),
                tool_result: Some(outcome.text),
                metadata: Some(metadata),
                created_at: issuer.line.timestamp.clone(),
            });
        }
    }
    Ok(())
}

fn recovered_prompts(
    batch: &mut CaptureBatchRequest,
    transcript: &[Line],
    session_key: &str,
    path: Option<&str>,
) -> Result<(), HookError> {
    for (position, line) in transcript.iter().enumerate() {
        if line.r#type != LineKind::User || line.tool_result_text().is_some() {
            continue;
        }
        let Some(content) = line.text() else {
            continue;
        };
        let metadata = line_metadata(
            path.map(str::to_owned),
            position,
            line.uuid.as_deref(),
            None,
            None,
            Some(true),
        )?;
        batch.messages.push(CaptureMessage {
            event_id: format!("{session_key}:u{}", position + 1),
            role: Role::User,
            content,
            tool_name: None,
            tool_args: None,
            tool_result: None,
            metadata: Some(metadata),
            created_at: line.timestamp.clone(),
        });
    }
    Ok(())
}

struct Issuer<'a> {
    line: &'a Line,
    input: &'a JsJson,
    name: &'a str,
}

fn tool_use_issuer<'a>(transcript: &'a [Line], outcome: &ToolOutcome) -> Option<Issuer<'a>> {
    let Some(tool_use_id) = &outcome.tool_use_id else {
        return None;
    };
    for line in transcript {
        for tool_use in line.tool_uses() {
            if tool_use.id == tool_use_id.as_str() {
                return Some(Issuer {
                    line,
                    input: tool_use.input,
                    name: tool_use.name,
                });
            }
        }
    }
    None
}

fn sidechain_flag(line: &Line) -> bool {
    line.is_sidechain.unwrap_or(false)
}

fn log_line(event: HookEvent, session_key: &str, totals: &Totals, now: DateTime<Utc>) -> String {
    if event.reads_transcript() {
        format!(
            "{} {} {}: updated conv {} — +{} msg (had {})",
            iso_at(now),
            event,
            session_key,
            totals.conversation_id,
            totals.inserted,
            totals.skipped
        )
    } else {
        format!(
            "{} {} {}: updated conv {} — +{} msg",
            iso_at(now),
            event,
            session_key,
            totals.conversation_id,
            totals.inserted
        )
    }
}
