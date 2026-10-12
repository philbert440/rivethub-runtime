use crate::Extra;
use crate::json::JsonObject;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use strum::{Display, EnumIter, EnumString};

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
    Display,
    EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum SessionHarness {
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
    Rivetos,
}

impl From<HarnessId> for SessionHarness {
    fn from(value: HarnessId) -> Self {
        match value {
            HarnessId::ClaudeCode => Self::ClaudeCode,
            HarnessId::GrokBuild => Self::GrokBuild,
            HarnessId::KimiCode => Self::KimiCode,
            HarnessId::Hermes => Self::Hermes,
            HarnessId::Codex => Self::Codex,
            HarnessId::Opencode => Self::Opencode,
            HarnessId::Pi => Self::Pi,
            HarnessId::QwenCode => Self::QwenCode,
            HarnessId::Cursor => Self::Cursor,
            HarnessId::Cowork => Self::Cowork,
        }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecision {
    Allow,
    Deny,
    AllowSession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalOutcome {
    Allow,
    Deny,
    AllowSession,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HarnessStatus {
    Working,
    Blocked,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusPhase {
    Thinking,
    Tool,
    Writing,
    Prompt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusSource {
    Herdr,
    Transcript,
    Hooks,
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum HarnessEvent {
    #[serde(rename_all = "camelCase")]
    AssistantDelta {
        session_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ReasoningDelta {
        session_id: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ToolUse {
        session_id: String,
        tool_call_id: String,
        name: String,
        input: JsonObject,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ToolResult {
        session_id: String,
        tool_call_id: String,
        name: String,
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalRequest {
        session_id: String,
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        name: String,
        input: JsonObject,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<Vec<ApprovalOption>>,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalResolved {
        session_id: String,
        request_id: String,
        decision: ApprovalOutcome,
    },
    #[serde(rename_all = "camelCase")]
    SessionCreated {
        session_id: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        supersedes: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    TurnComplete {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Error {
        session_id: String,
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery_id: Option<String>,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retryable: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    SessionUpdated {
        session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        previous_session_id: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalOption {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "status", rename_all = "camelCase")]
pub struct HarnessStatusFrame {
    pub session_id: String,
    pub status: HarnessStatus,
    pub since: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<StatusSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<StatusPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<StatusTool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCapabilities {
    #[serde(flatten)]
    pub control: HarnessControlCapabilities,
    #[serde(flatten)]
    pub observation: HarnessObservationCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_options: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_model: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_attachments: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<HarnessModelOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_source: Option<HarnessModelsSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<EffortOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_flag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessControlCapabilities {
    pub interrupt: bool,
    pub resume: bool,
    pub approvals: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessObservationCapabilities {
    pub live_stream: bool,
    pub list_sessions: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffortOption {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HarnessModelsSource {
    Discovered,
    Config,
    Merged,
    Static,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessModelOption {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_modalities: Option<Vec<String>>,
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub efforts: Option<Vec<EffortOption>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HarnessSessionStatus {
    Active,
    Idle,
    Ended,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSessionSummary {
    pub session_id: String,
    pub harness_id: HarnessId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: HarnessSessionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Transport {
    Protocol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessSessionList {
    pub sessions: Vec<HarnessSessionSummary>,
}
