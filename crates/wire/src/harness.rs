use crate::Extra;
use crate::events::ContextSource;
use crate::gateway::sessions::HarnessTranscriptTurn;
use crate::json::JsonObject;
use crate::session_id::SessionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::fmt;
use std::str::FromStr;
use strum::{Display, EnumIter, EnumString};
use thiserror::Error;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    Display,
    EnumString,
    EnumIter,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum HarnessId {
    ClaudeCode,
    GrokBuild,
    KimiCode,
    Hermes,
    Codex,
    Opencode,
    Pi,
    QwenCode,
    Cursor,
    Cowork,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub enum SessionHarness {
    Harness(HarnessId),
    Rivetos,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown harness id: {0}")]
pub struct SessionHarnessError(String);

impl TryFrom<String> for SessionHarness {
    type Error = SessionHarnessError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value == "rivetos" {
            return Ok(Self::Rivetos);
        }
        HarnessId::from_str(&value)
            .map(Self::Harness)
            .map_err(|_| SessionHarnessError(value))
    }
}

impl From<SessionHarness> for String {
    fn from(value: SessionHarness) -> Self {
        value.to_string()
    }
}

impl fmt::Display for SessionHarness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Harness(harness) => harness.fmt(f),
            Self::Rivetos => f.write_str("rivetos"),
        }
    }
}

impl From<HarnessId> for SessionHarness {
    fn from(value: HarnessId) -> Self {
        Self::Harness(value)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    Display,
    EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum RosterCommand {
    Claude,
    Grok,
    Kimi,
    Opencode,
    Hermes,
    Codex,
    Pi,
    Qwen,
    Cursor,
    Cowork,
}

impl From<HarnessId> for RosterCommand {
    fn from(harness: HarnessId) -> Self {
        match harness {
            HarnessId::ClaudeCode => Self::Claude,
            HarnessId::GrokBuild => Self::Grok,
            HarnessId::KimiCode => Self::Kimi,
            HarnessId::Hermes => Self::Hermes,
            HarnessId::Codex => Self::Codex,
            HarnessId::Opencode => Self::Opencode,
            HarnessId::Pi => Self::Pi,
            HarnessId::QwenCode => Self::Qwen,
            HarnessId::Cursor => Self::Cursor,
            HarnessId::Cowork => Self::Cowork,
        }
    }
}

impl From<RosterCommand> for HarnessId {
    fn from(command: RosterCommand) -> Self {
        match command {
            RosterCommand::Claude => Self::ClaudeCode,
            RosterCommand::Grok => Self::GrokBuild,
            RosterCommand::Kimi => Self::KimiCode,
            RosterCommand::Hermes => Self::Hermes,
            RosterCommand::Codex => Self::Codex,
            RosterCommand::Opencode => Self::Opencode,
            RosterCommand::Pi => Self::Pi,
            RosterCommand::Qwen => Self::QwenCode,
            RosterCommand::Cursor => Self::Cursor,
            RosterCommand::Cowork => Self::Cowork,
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    Display,
    EnumString,
    EnumIter,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum HarnessCode {
    InvalidSessionId,
    BadRequest,
    SessionIdCollision,
    TurnInFlight,
    CapabilityUnsupported,
    UnknownApproval,
    UnknownPrompt,
}

impl HarnessCode {
    #[must_use]
    pub const fn status(self) -> u16 {
        match self {
            Self::InvalidSessionId | Self::BadRequest => 400,
            Self::UnknownApproval | Self::UnknownPrompt => 404,
            Self::SessionIdCollision | Self::TurnInFlight => 409,
            Self::CapabilityUnsupported => 501,
        }
    }

    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(self, Self::TurnInFlight)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecision {
    Allow,
    Deny,
    AllowSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalOutcome {
    Allow,
    Deny,
    AllowSession,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HarnessStatus {
    Working,
    Blocked,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StatusPhase {
    Thinking,
    Tool,
    Writing,
    Prompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StatusSource {
    Herdr,
    Transcript,
    Hooks,
    Protocol,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HarnessEvent {
    #[serde(rename_all = "camelCase")]
    AssistantDelta {
        session_id: SessionId,
        text: String,
        turn_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ReasoningDelta {
        session_id: SessionId,
        text: String,
        turn_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ToolUse {
        session_id: SessionId,
        tool_call_id: String,
        name: String,
        input: JsonObject,
        turn_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ToolResult {
        session_id: SessionId,
        tool_call_id: String,
        name: String,
        output: String,
        is_error: Option<bool>,
        turn_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalRequest {
        session_id: SessionId,
        request_id: String,
        tool_call_id: Option<String>,
        name: String,
        input: JsonObject,
        reason: Option<String>,
        options: Option<Vec<ApprovalOption>>,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalResolved {
        session_id: SessionId,
        request_id: String,
        decision: ApprovalOutcome,
    },
    #[serde(rename_all = "camelCase")]
    SessionCreated {
        session_id: SessionId,
        summary: Box<HarnessSessionSummary>,
        supersedes: Option<SessionId>,
    },
    #[serde(rename_all = "camelCase")]
    TurnComplete {
        session_id: SessionId,
        turn_id: Option<String>,
        stop_reason: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Error {
        session_id: SessionId,
        code: String,
        delivery_id: Option<String>,
        message: String,
        retryable: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    SessionUpdated {
        session_id: SessionId,
        status: HarnessSessionStatus,
        previous_session_id: Option<SessionId>,
        supersedes: Option<SessionId>,
        blocked: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    Transcript {
        session_id: SessionId,
        rev: u64,
        from: u64,
        total: u64,
        turns: Vec<HarnessTranscriptTurn>,
        command: RosterCommand,
        truncated_before: Option<bool>,
        context_window: Option<u64>,
        compact_at: Option<u64>,
        context_source: Option<ContextSource>,
    },
    Status(HarnessStatusFrame),
    #[serde(rename_all = "camelCase")]
    Prompt {
        session_id: SessionId,
        prompt_id: String,
        kind: PromptKind,
        tool_name: String,
        questions: Vec<PromptQuestion>,
        resolved: Option<PromptResolution>,
        screen: Option<PromptScreen>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum PromptKind {
    AskUser,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptQuestion {
    pub question: Option<String>,
    pub header: Option<String>,
    pub multi_select: bool,
    pub free_text: Option<bool>,
    pub options: Vec<PromptOption>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptOption {
    pub label: String,
    pub description: Option<String>,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptResolution {
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub at: DateTime<Utc>,
    pub answer_text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PromptScreen {
    pub current: u32,
    pub total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalOption {
    pub key: String,
    pub label: String,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessStatusFrame {
    pub session_id: SessionId,
    pub status: HarnessStatus,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub since: DateTime<Utc>,
    pub source: Option<StatusSource>,
    pub phase: Option<StatusPhase>,
    pub tool: Option<StatusTool>,
    pub prompt_id: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusTool {
    pub name: String,
    pub tool_call_id: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCapabilities {
    #[serde(flatten)]
    pub control: HarnessControlCapabilities,
    #[serde(flatten)]
    pub observation: HarnessObservationCapabilities,
    pub turn_options: Option<bool>,
    pub launch_model: Option<bool>,
    pub image_attachments: Option<bool>,
    pub models: Option<Vec<HarnessModelOption>>,
    pub models_source: Option<HarnessModelsSource>,
    pub efforts: Option<Vec<EffortOption>>,
    pub model_flag: Option<String>,
    pub effort_flag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessControlCapabilities {
    pub interrupt: bool,
    pub resume: bool,
    pub approvals: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessObservationCapabilities {
    pub live_stream: bool,
    pub list_sessions: bool,
    pub drive: Option<bool>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffortOption {
    pub id: String,
    pub label: String,
    pub is_default: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HarnessModelsSource {
    Discovered,
    Config,
    Merged,
    Static,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessModelOption {
    pub input_modalities: Option<Vec<String>>,
    pub id: String,
    pub label: String,
    pub is_default: Option<bool>,
    pub efforts: Option<Vec<EffortOption>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HarnessSessionStatus {
    Active,
    Idle,
    Ended,
    Error,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSessionSummary {
    pub session_id: SessionId,
    pub harness_id: HarnessId,
    pub title: Option<String>,
    pub cwd: Option<String>,
    #[serde_as(as = "crate::time::Millis")]
    pub created_at: DateTime<Utc>,
    #[serde_as(as = "crate::time::Millis")]
    pub updated_at: DateTime<Utc>,
    pub status: HarnessSessionStatus,
    pub blocked: Option<bool>,
    pub supersedes: Option<SessionId>,
    pub model: Option<String>,
    pub parent_session_id: Option<SessionId>,
    pub agent_name: Option<String>,
    pub task_id: Option<String>,
    pub effort: Option<String>,
    pub transport: Option<Transport>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Transport {
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSessionList {
    pub sessions: Vec<HarnessSessionSummary>,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessStoreSession {
    pub id: String,
    pub command: RosterCommand,
    pub title: String,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub updated_at: DateTime<Utc>,
    #[serde(skip)]
    pub created_at: Option<DateTime<Utc>>,
    pub parent_session_id: Option<String>,
    pub agent_name: Option<String>,
    pub model: Option<String>,
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessStoreSessionList {
    pub sessions: Vec<HarnessStoreSession>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessOffering {
    pub harness_id: HarnessId,
    pub chat: bool,
    pub capabilities: HarnessCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessOfferingList {
    pub harnesses: Vec<HarnessOffering>,
}
