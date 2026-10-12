use crate::{JsonObject, message::Role, message::ToolCall};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use strum::{Display, EnumIter, EnumString};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamEventKind {
    Text,
    Reasoning,
    ToolStart,
    ToolResult,
    Status,
    Interrupt,
    Done,
    Error,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub r#type: StreamEventKind,
    pub content: String,
    pub metadata: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub timestamp: i64,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationRequest {
    pub from_agent: String,
    pub to_agent: String,
    pub task: String,
    pub context: Option<Vec<String>>,
    pub timeout_ms: Option<u64>,
    pub no_delegation: Option<bool>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DelegationStatus {
    Completed,
    Failed,
    Timeout,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationResult {
    pub status: DelegationStatus,
    pub response: String,
    pub iterations: Option<u32>,
    pub usage: Option<TokenUsage>,
    pub tools_used: Option<Vec<String>>,
    pub duration_ms: Option<u64>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub reasoning_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmResponseKind {
    Text,
    ToolCalls,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmResponse {
    pub r#type: LlmResponseKind,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub usage: Option<LlmUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmChunkKind {
    Text,
    Reasoning,
    ToolCallStart,
    ToolCallDelta,
    ToolCallDone,
    Status,
    Done,
    Error,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialToolCall {
    pub index: Option<u32>,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<JsonObject>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmChunk {
    pub r#type: LlmChunkKind,
    pub delta: Option<String>,
    pub tool_call: Option<PartialToolCall>,
    pub usage: Option<LlmUsage>,
    pub citations: Option<Vec<String>>,
    pub error: Option<String>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString, EnumIter,
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
    Xhigh,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContext {
    pub session_token: String,
    pub agent_id: String,
    pub node_id: String,
    pub conversation_id: String,
    pub user_id: String,
    pub discord_channel_id: Option<String>,
    pub telegram_chat_id: Option<String>,
    pub parent_session_id: Option<String>,
    pub allowlist: Vec<String>,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub issued_at: DateTime<Utc>,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub expires_at: DateTime<Utc>,
    pub working_dir: Option<String>,
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorSeverity {
    Fatal,
    Error,
    Warning,
    Transient,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RivetError {
    pub name: String,
    pub code: String,
    pub message: String,
    pub severity: ErrorSeverity,
    pub retryable: bool,
    pub timestamp: i64,
    pub context: JsonObject,
    pub cause: Option<String>,
    pub stack: String,
}

pub mod openai {
    use crate::JsonObject;
    use crate::message::Role;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    pub enum Tool {
        #[serde(rename = "function")]
        Function { function: FunctionDef },
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct FunctionDef {
        pub name: String,
        pub description: String,
        pub parameters: JsonObject,
    }

    #[serde_with::skip_serializing_none]
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct FunctionDelta {
        pub name: Option<String>,
        pub arguments: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    pub enum MessageToolCall {
        #[serde(rename = "function")]
        Function(StreamedPlainToolCall),
    }

    #[serde_with::skip_serializing_none]
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Message {
        pub role: Role,
        pub content: Option<String>,
        pub reasoning_content: Option<String>,
        pub tool_calls: Option<Vec<MessageToolCall>>,
        pub tool_call_id: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct StreamOptions {
        pub include_usage: bool,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ChatTemplateKwargs {
        pub enable_thinking: bool,
    }

    #[serde_with::skip_serializing_none]
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct ChatCompletionRequest {
        pub model: String,
        pub max_tokens: Option<u64>,
        pub temperature: Option<f64>,
        pub top_p: Option<f64>,
        pub messages: Vec<Message>,
        pub tools: Option<Vec<Tool>>,
        pub tool_choice: Option<String>,
        pub stream: Option<bool>,
        pub stream_options: Option<StreamOptions>,
        pub chat_template_kwargs: Option<ChatTemplateKwargs>,
        pub think: Option<bool>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum FinishReason {
        Stop,
        Length,
        ToolCalls,
        ContentFilter,
    }

    #[serde_with::skip_serializing_none]
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StreamedPlainToolCall {
        pub index: Option<u32>,
        pub id: Option<String>,
        pub function: FunctionDelta,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    pub enum FunctionToolCallTag {
        #[serde(rename = "function")]
        Function(StreamedPlainToolCall),
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(untagged)]
    pub enum StreamedToolCall {
        Function(FunctionToolCallTag),
        Plain(StreamedPlainToolCall),
    }

    #[serde_with::skip_serializing_none]
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Delta {
        pub role: Option<Role>,
        pub content: Option<String>,
        pub reasoning_content: Option<String>,
        pub tool_calls: Option<Vec<StreamedToolCall>>,
    }

    #[serde_with::skip_serializing_none]
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Choice {
        pub index: Option<u64>,
        pub delta: Delta,
        pub logprobs: Option<JsonObject>,
        pub finish_reason: Option<FinishReason>,
        pub stop_reason: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct PromptTokensDetails {
        pub cached_tokens: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct CompletionTokensDetails {
        pub reasoning_tokens: u64,
    }

    #[serde_with::skip_serializing_none]
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Usage {
        pub prompt_tokens: u64,
        pub completion_tokens: u64,
        pub total_tokens: u64,
        pub prompt_tokens_details: Option<PromptTokensDetails>,
        pub completion_tokens_details: Option<CompletionTokensDetails>,
    }

    #[serde_with::skip_serializing_none]
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct Timings {
        pub cache_n: u64,
        pub prompt_n: u64,
        pub prompt_ms: f64,
        pub prompt_per_token_ms: f64,
        pub prompt_per_second: u64,
        pub predicted_n: u64,
        pub predicted_ms: f64,
        pub predicted_per_token_ms: f64,
        pub predicted_per_second: f64,
    }

    #[serde_with::skip_serializing_none]
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct ChatCompletionChunk {
        pub id: String,
        pub object: String,
        pub created: i64,
        pub model: String,
        pub system_fingerprint: Option<String>,
        pub choices: Vec<Choice>,
        pub usage: Option<Usage>,
        pub timings: Option<Timings>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ModelList {
        pub object: String,
        pub data: Vec<Model>,
    }

    #[serde_with::skip_serializing_none]
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Model {
        pub id: String,
        pub object: String,
        pub created: i64,
        pub owned_by: String,
        pub root: Option<String>,
        pub parent: Option<String>,
        pub max_model_len: Option<u64>,
        pub meta: Option<JsonObject>,
        pub permission: Option<Vec<JsonObject>>,
    }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaMessage>,
    pub think: Option<bool>,
    pub tools: Option<Vec<openai::Tool>>,
    pub tool_choice: Option<String>,
    pub stream: Option<bool>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaPlainToolCall {
    pub id: Option<String>,
    pub function: OllamaMessageFunction,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FunctionToolCallTagOllama {
    #[serde(rename = "function")]
    Function {
        id: Option<String>,
        function: OllamaMessageFunction,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum OllamaToolCall {
    Function(FunctionToolCallTagOllama),
    Plain(OllamaPlainToolCall),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaMessageFunction {
    pub name: String,
    pub arguments: JsonObject,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaMessage {
    pub role: Role,
    pub content: String,
    pub thinking: Option<String>,
    pub tool_calls: Option<Vec<OllamaToolCall>>,
    pub tool_call_id: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatChunk {
    pub model: String,
    pub created_at: String,
    pub message: OllamaMessage,
    pub done: bool,
    pub done_reason: Option<String>,
    pub total_duration: Option<u64>,
    pub load_duration: Option<u64>,
    pub prompt_eval_count: Option<u64>,
    pub prompt_eval_duration: Option<u64>,
    pub eval_count: Option<u64>,
    pub eval_duration: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaTags {
    pub models: Vec<OllamaModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaModel {
    pub name: String,
    pub model: String,
    pub modified_at: DateTime<Utc>,
    pub size: u64,
    pub digest: String,
    pub details: OllamaModelDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaModelDetails {
    pub parent_model: String,
    pub format: String,
    pub family: String,
    pub families: Vec<String>,
    pub parameter_size: String,
    pub quantization_level: String,
}
