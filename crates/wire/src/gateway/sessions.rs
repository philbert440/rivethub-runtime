use crate::events::ContextSource;
use crate::harness::{HarnessId, HarnessSessionSummary};
use crate::json::JsonObject;
use crate::message::Role;
use crate::session_id::SessionId;
use crate::stream::StreamEvent;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptTurn {
    pub role: Role,
    pub text: String,
    #[serde(default)]
    #[serde_as(as = "Option<serde_with::TimestampMilliSeconds>")]
    pub ts: Option<DateTime<Utc>>,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptResponse {
    pub id: String,
    pub command: String,
    pub turns: Vec<TranscriptTurn>,
    pub context_window: Option<u64>,
    pub compact_at: Option<u64>,
    pub context_source: Option<ContextSource>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptBlock {
    Thinking,
    Text,
    ToolUse,
    ToolResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptToolStatus {
    Running,
    Done,
    Error,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessTranscriptTool {
    pub name: String,
    pub status: TranscriptToolStatus,
    pub args: Option<JsonObject>,
    pub id: Option<String>,
    pub input: Option<JsonObject>,
    pub result_text: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessTranscriptTurn {
    pub role: SessionMessageRole,
    pub text: String,
    pub thinking: Option<String>,
    pub tools: Option<Vec<HarnessTranscriptTool>>,
    pub usage: Option<MessageUsage>,
    pub model: Option<String>,
    pub stop_reason: Option<String>,
    pub last_block: Option<TranscriptBlock>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub complete: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub compact: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSessionTranscriptResponse {
    pub session_id: SessionId,
    pub harness_id: HarnessId,
    pub turns: Vec<HarnessTranscriptTurn>,
    pub context_window: u64,
    pub compact_at: u64,
    pub context_source: ContextSource,
    pub redirected_to: Option<SessionId>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessTurnAccepted {
    pub ok: bool,
    pub session_id: SessionId,
    pub redirected_to: Option<SessionId>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dismissed_dialog: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSessionResponse {
    #[serde(flatten)]
    pub summary: HarnessSessionSummary,
    pub redirected_to: Option<SessionId>,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub last_active: DateTime<Utc>,
    pub messages: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionList {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageList {
    pub messages: Vec<SessionMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionMessageRole {
    User,
    Assistant,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub id: String,
    pub session_id: String,
    pub role: SessionMessageRole,
    pub text: String,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub ts: DateTime<Utc>,
    pub usage: Option<MessageUsage>,
    pub model: Option<String>,
    pub duration_ms: Option<u64>,
    pub thinking: Option<bool>,
    pub tools: Option<Vec<String>>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPostRequest {
    pub exclude_tools: Option<Vec<String>>,
    pub text: String,
    pub user_id: Option<String>,
    pub agent: Option<String>,
    pub thinking: Option<crate::stream::ThinkingLevel>,
    pub system_prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPostAccepted {
    pub accepted: bool,
    pub session: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionPostReply {
    pub message: SessionMessage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionWsClientMessage {
    pub r#type: SessionClientKind,
    pub session: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionClientKind {
    Watch,
    Unwatch,
    Sync,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SessionWsFrame {
    Message {
        #[serde(flatten)]
        message: SessionMessage,
    },
    Stream {
        session: String,
        event: StreamEvent,
    },
    Transcript(TranscriptWsFrame),
    #[serde(rename = "sessions-dirty")]
    SessionsDirty,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptWsFrame {
    pub session: String,
    pub rev: u64,
    pub from: u64,
    pub turns: Vec<HarnessTranscriptTurn>,
    pub total: u64,
    pub command: String,
    pub truncated_before: Option<bool>,
}
