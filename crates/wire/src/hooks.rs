use crate::JsonObject;
use crate::message::Message;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookEventName {
    #[serde(rename = "provider:before")]
    ProviderBefore,
    #[serde(rename = "provider:after")]
    ProviderAfter,
    #[serde(rename = "provider:error")]
    ProviderError,
    #[serde(rename = "tool:before")]
    ToolBefore,
    #[serde(rename = "tool:after")]
    ToolAfter,
    #[serde(rename = "session:start")]
    SessionStart,
    #[serde(rename = "session:end")]
    SessionEnd,
    #[serde(rename = "turn:before")]
    TurnBefore,
    #[serde(rename = "turn:after")]
    TurnAfter,
    #[serde(rename = "turn:reflect")]
    TurnReflect,
    #[serde(rename = "skill:before")]
    SkillBefore,
    #[serde(rename = "skill:after")]
    SkillAfter,
    #[serde(rename = "compact:before")]
    CompactBefore,
    #[serde(rename = "compact:after")]
    CompactAfter,
    #[serde(rename = "delegation:before")]
    DelegationBefore,
    #[serde(rename = "delegation:after")]
    DelegationAfter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookContextBase {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub timestamp: i64,
    pub metadata: JsonObject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReflectionSource {
    Complexity,
    Periodic,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DelegationHookStatus {
    Completed,
    Failed,
    Timeout,
    Cached,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTokens {
    pub prompt: u64,
    pub completion: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnComplexity {
    pub tool_call_count: u64,
    pub had_error_recovery: bool,
    pub had_user_correction: bool,
    pub unique_tools: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookContext {
    #[serde(flatten)]
    pub base: HookContextBase,
    #[serde(flatten)]
    pub kind: HookContextKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event")]
pub enum HookContextKind {
    #[serde(rename = "provider:before", rename_all = "camelCase")]
    ProviderBefore {
        provider_id: String,
        model: String,
        messages: Vec<Message>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skip: Option<bool>,
    },
    #[serde(rename = "provider:after", rename_all = "camelCase")]
    ProviderAfter {
        provider_id: String,
        model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<HookUsage>,
        latency_ms: i64,
        has_tool_calls: bool,
    },
    #[serde(rename = "provider:error", rename_all = "camelCase")]
    ProviderError {
        provider_id: String,
        model: String,
        error: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status_code: Option<u16>,
    },
    #[serde(rename = "tool:before", rename_all = "camelCase")]
    ToolBefore {
        tool_name: String,
        args: JsonObject,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocked: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block_reason: Option<String>,
    },
    #[serde(rename = "tool:after", rename_all = "camelCase")]
    ToolAfter {
        tool_name: String,
        args: JsonObject,
        result: JsonObject,
        duration_ms: i64,
        is_error: bool,
    },
    #[serde(rename = "session:start", rename_all = "camelCase")]
    SessionStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        platform: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_id: Option<String>,
    },
    #[serde(rename = "session:end", rename_all = "camelCase")]
    SessionEnd {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_count: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total_tokens: Option<SessionTokens>,
    },
    #[serde(rename = "turn:before", rename_all = "camelCase")]
    TurnBefore {
        user_message: Message,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skip: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skip_reason: Option<String>,
    },
    #[serde(rename = "turn:after", rename_all = "camelCase")]
    TurnAfter {
        response: String,
        tools_used: Vec<String>,
        iterations: u32,
        aborted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<HookUsage>,
    },
    #[serde(rename = "turn:reflect", rename_all = "camelCase")]
    TurnReflect {
        response: String,
        tools_used: Vec<String>,
        iterations: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<HookUsage>,
        complexity: TurnComplexity,
        source: ReflectionSource,
        user_message: Message,
    },
    #[serde(rename = "skill:before", rename_all = "camelCase")]
    SkillBefore {
        skill_name: String,
        skill_location: String,
        matched_triggers: Vec<String>,
        match_score: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skip: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skip_reason: Option<String>,
    },
    #[serde(rename = "skill:after", rename_all = "camelCase")]
    SkillAfter {
        skill_name: String,
        success: bool,
        tools_used: Vec<String>,
        iterations: u32,
        duration_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<HookUsage>,
    },
    #[serde(rename = "compact:before", rename_all = "camelCase")]
    CompactBefore { message_count: u64 },
    #[serde(rename = "compact:after", rename_all = "camelCase")]
    CompactAfter {
        remaining_messages: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    #[serde(rename = "delegation:before", rename_all = "camelCase")]
    DelegationBefore {
        from_agent: String,
        to_agent: String,
        task: String,
        chain_depth: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blocked: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        block_reason: Option<String>,
    },
    #[serde(rename = "delegation:after", rename_all = "camelCase")]
    DelegationAfter {
        from_agent: String,
        to_agent: String,
        task: String,
        status: DelegationHookStatus,
        duration_ms: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<HookUsage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tools_used: Option<Vec<String>>,
        cached: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookPipelineError {
    pub hook_id: String,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookPipelineResult {
    pub context: HookContext,
    pub aborted: bool,
    pub skipped: bool,
    pub errors: Vec<HookPipelineError>,
    pub ran: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HookErrorMode {
    Continue,
    Abort,
    Retry,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HookConfig {
    pub id: String,
    pub event: HookEventName,
    pub r#type: HookConfigKind,
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_error: Option<HookErrorMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_filter: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_filter: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HookConfigKind {
    Shell,
    Http,
    Internal,
}
