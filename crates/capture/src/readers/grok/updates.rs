use std::collections::BTreeMap;

use chrono::DateTime;
use chrono::SecondsFormat;
use kernel::transcript::TranscriptReaderError;
use serde::Deserialize;
use wire::JsonObject;
use wire::message::Role;
use wire::session_id::SessionId;

use super::FORMAT;
use super::output::ContentItem;
use super::output::tool_result;
use super::turns::TurnLedger;
use crate::readers::message::Mapped;
use crate::readers::message::SkipRule;
use crate::readers::message::SkippedRecord;
use crate::readers::message::TextTurn;
use crate::readers::message::ToolFields;
use crate::readers::message::TranscriptSlice;
use crate::readers::message::assemble;
use crate::readers::message::parse_json;
use crate::readers::message::push_mapped;
use crate::readers::message::text_turn;
use crate::readers::message::tool_turn;
use crate::readers::message::unknown;
use crate::readers::scan::terminated_lines;

pub(super) fn map_updates(
    session: &SessionId,
    text: &str,
    offset: u64,
) -> Result<TranscriptSlice, TranscriptReaderError> {
    let (records, last) = terminated_lines(text)?;
    let mut reading = Reading {
        session,
        ledger: TurnLedger::default(),
        opened: BTreeMap::new(),
    };
    let mut drafts = Vec::new();
    let mut skipped = Vec::new();
    for record in &records {
        let mapped = match parse_json::<UpdateLine>(record.number, &record.text) {
            Ok(line) => reading.line(record.number, line)?,
            Err(record) => Mapped::Skip(record),
        };
        push_mapped(mapped, &mut drafts, &mut skipped);
    }
    let settled = reading.ledger.settled(last);
    drafts.retain(|draft| draft.position <= settled);
    skipped.retain(|record| record.position <= settled);
    Ok(assemble(offset, settled, drafts, skipped))
}

struct Reading<'a> {
    session: &'a SessionId,
    ledger: TurnLedger,
    opened: BTreeMap<String, OpenedCall>,
}

struct OpenedCall {
    name: Option<String>,
    input: Option<JsonObject>,
}

impl Reading<'_> {
    fn line(&mut self, position: u64, line: UpdateLine) -> Result<Mapped, TranscriptReaderError> {
        let Params { meta, update } = line.params;
        if let Some(prompt) = &meta.prompt_id {
            self.ledger.prompt(prompt, position);
        }
        let at = meta.agent_timestamp_ms;
        match update {
            SessionUpdate::UserMessageChunk { content } => {
                self.ledger.user(position);
                self.text(position, at, Role::User, content.text())
            }
            SessionUpdate::AgentMessageChunk { content } => {
                self.text(position, at, Role::Assistant, content.text())
            }
            SessionUpdate::AgentThoughtChunk { content } => {
                let thought = content.text().map(|text| format!("[thinking] {text}"));
                self.text(position, at, Role::Assistant, thought.as_deref())
            }
            SessionUpdate::ToolCall(call) => {
                self.opened.insert(
                    call.tool_call_id,
                    OpenedCall {
                        name: call.title,
                        input: call.raw_input,
                    },
                );
                Ok(Mapped::None)
            }
            SessionUpdate::ToolCallUpdate(update) => self.tool(position, at, update),
            SessionUpdate::TurnCompleted { prompt_id } => {
                self.ledger.completed(&prompt_id, position);
                Ok(Mapped::None)
            }
            SessionUpdate::MemoryFlushStarted => self.text(
                position,
                at,
                Role::System,
                Some("[grok.memory_flush_started]"),
            ),
            SessionUpdate::MemoryFlushCompleted => self.text(
                position,
                at,
                Role::System,
                Some("[grok.memory_flush_completed]"),
            ),
            SessionUpdate::Unknown => Ok(Mapped::Skip(unknown(
                position,
                "unknown grok session update",
            ))),
            SessionUpdate::HookExecution
            | SessionUpdate::AvailableCommandsUpdate
            | SessionUpdate::Plan
            | SessionUpdate::BackgroundTasks
            | SessionUpdate::TaskBackgrounded
            | SessionUpdate::TaskCompleted
            | SessionUpdate::RetryState
            | SessionUpdate::AutoCompactStarted
            | SessionUpdate::CompactionCheckpoint
            | SessionUpdate::AutoCompactCompleted
            | SessionUpdate::SessionRecap
            | SessionUpdate::SubagentSpawned
            | SessionUpdate::SubagentFinished
            | SessionUpdate::GoalUpdated
            | SessionUpdate::MemoryDreamQueued
            | SessionUpdate::MemoryDreamStarted
            | SessionUpdate::MemoryDreamCompleted => Ok(Mapped::None),
        }
    }

    fn text(
        &self,
        position: u64,
        at: Option<i64>,
        role: Role,
        content: Option<&str>,
    ) -> Result<Mapped, TranscriptReaderError> {
        let Some(content) = content else {
            return Ok(Mapped::Skip(unknown(position, "grok content is not text")));
        };
        let at = match instant(position, at) {
            Ok(at) => at,
            Err(record) => return Ok(Mapped::Skip(record)),
        };
        text_turn(&TextTurn {
            session: self.session,
            position,
            role,
            content,
            at: &at,
            format: FORMAT,
        })
    }

    fn tool(
        &mut self,
        position: u64,
        at: Option<i64>,
        update: ToolUpdate,
    ) -> Result<Mapped, TranscriptReaderError> {
        if update.status != Some(ToolStatus::Completed) {
            return Ok(Mapped::None);
        }
        let (name, input) = match self.opened.remove(&update.tool_call_id) {
            Some(call) => (call.name.or(update.title), call.input.or(update.raw_input)),
            None => (update.title, update.raw_input),
        };
        let result = match tool_result(position, update.raw_output, &update.content) {
            Ok(result) => result,
            Err(record) => return Ok(Mapped::Skip(record)),
        };
        let at = match instant(position, at) {
            Ok(at) => at,
            Err(record) => return Ok(Mapped::Skip(record)),
        };
        let content = match &name {
            Some(name) => format!("[tool] {name}"),
            None => "[tool]".to_owned(),
        };
        tool_turn(
            &TextTurn {
                session: self.session,
                position,
                role: Role::Tool,
                content: &content,
                at: &at,
                format: FORMAT,
            },
            ToolFields {
                name,
                arguments: input,
                result,
            },
        )
    }
}

fn instant(position: u64, millis: Option<i64>) -> Result<String, SkippedRecord> {
    millis
        .and_then(DateTime::from_timestamp_millis)
        .map(|at| at.to_rfc3339_opts(SecondsFormat::Millis, true))
        .ok_or_else(|| SkippedRecord {
            position,
            rule: SkipRule::UnparseableTimestamp,
            detail: "agentTimestampMs is missing or out of range".to_owned(),
        })
}

#[derive(Debug, Deserialize)]
struct UpdateLine {
    params: Params,
}

#[derive(Debug, Deserialize)]
struct Params {
    #[serde(rename = "_meta", default)]
    meta: LineMeta,
    update: SessionUpdate,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LineMeta {
    agent_timestamp_ms: Option<i64>,
    prompt_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "sessionUpdate", rename_all = "snake_case")]
enum SessionUpdate {
    UserMessageChunk {
        content: AcpContent,
    },
    AgentMessageChunk {
        content: AcpContent,
    },
    AgentThoughtChunk {
        content: AcpContent,
    },
    ToolCall(CallStart),
    ToolCallUpdate(ToolUpdate),
    TurnCompleted {
        prompt_id: String,
    },
    MemoryFlushStarted,
    MemoryFlushCompleted,
    HookExecution,
    AvailableCommandsUpdate,
    Plan,
    BackgroundTasks,
    TaskBackgrounded,
    TaskCompleted,
    RetryState,
    AutoCompactStarted,
    CompactionCheckpoint,
    AutoCompactCompleted,
    SessionRecap,
    SubagentSpawned,
    SubagentFinished,
    GoalUpdated,
    MemoryDreamQueued,
    MemoryDreamStarted,
    MemoryDreamCompleted,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallStart {
    tool_call_id: String,
    title: Option<String>,
    raw_input: Option<JsonObject>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolUpdate {
    tool_call_id: String,
    status: Option<ToolStatus>,
    title: Option<String>,
    raw_input: Option<JsonObject>,
    raw_output: Option<JsonObject>,
    #[serde(default)]
    content: Vec<ContentItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ToolStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum AcpContent {
    Text {
        text: String,
    },
    #[serde(other)]
    Other,
}

impl AcpContent {
    pub(super) fn text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Other => None,
        }
    }
}
