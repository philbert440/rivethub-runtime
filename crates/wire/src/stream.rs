use crate::{JsonObject, message::Role, message::ToolCall};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamEvent {
    pub r#type: StreamEventKind,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationRequest {
    pub from_agent: String,
    pub to_agent: String,
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_delegation: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DelegationStatus {
    Completed,
    Failed,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DelegationResult {
    pub status: DelegationStatus,
    pub response: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iterations: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools_used: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LlmResponseKind {
    Text,
    ToolCalls,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmResponse {
    pub r#type: LlmResponseKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartialToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmChunk {
    pub r#type: LlmChunkKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<PartialToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<LlmUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citations: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingLevel {
    Off,
    Low,
    Medium,
    High,
    Xhigh,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContext {
    pub session_token: String,
    pub agent_id: String,
    pub node_id: String,
    pub conversation_id: String,
    pub user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telegram_chat_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
    pub allowlist: Vec<String>,
    pub issued_at: i64,
    pub expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct FunctionDelta {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub arguments: Option<String>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "camelCase")]
    pub enum MessageToolCall {
        #[serde(rename = "function")]
        Function(StreamedPlainToolCall),
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Message {
        pub role: Role,
        pub content: Option<String>,
        #[serde(
            rename = "reasoning_content",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        pub reasoning_content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tool_calls: Option<Vec<MessageToolCall>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
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

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct ChatCompletionRequest {
        pub model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max_tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub temperature: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub top_p: Option<f64>,
        pub messages: Vec<Message>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tools: Option<Vec<Tool>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tool_choice: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub stream: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub stream_options: Option<StreamOptions>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub chat_template_kwargs: Option<ChatTemplateKwargs>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
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

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "camelCase")]
    pub struct StreamedPlainToolCall {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub index: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
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

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Delta {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub role: Option<Role>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub reasoning_content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub tool_calls: Option<Vec<StreamedToolCall>>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Choice {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub index: Option<u64>,
        pub delta: Delta,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub logprobs: Option<JsonObject>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub finish_reason: Option<FinishReason>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
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

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub struct Usage {
        pub prompt_tokens: u64,
        pub completion_tokens: u64,
        pub total_tokens: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub prompt_tokens_details: Option<PromptTokensDetails>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub completion_tokens_details: Option<CompletionTokensDetails>,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
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

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    pub struct ChatCompletionChunk {
        pub id: String,
        pub object: String,
        pub created: i64,
        pub model: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub system_fingerprint: Option<String>,
        pub choices: Vec<Choice>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub usage: Option<Usage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub timings: Option<Timings>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ModelList {
        pub object: String,
        pub data: Vec<Model>,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct Model {
        pub id: String,
        pub object: String,
        pub created: i64,
        pub owned_by: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub root: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub parent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub max_model_len: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub meta: Option<JsonObject>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub permission: Option<Vec<JsonObject>>,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<openai::Tool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaPlainToolCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub function: OllamaMessageFunction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FunctionToolCallTagOllama {
    #[serde(rename = "function")]
    Function {
        #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaMessage {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OllamaToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaChatChunk {
    pub model: String,
    pub created_at: String,
    pub message: OllamaMessage,
    pub done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub done_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_duration: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_duration: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_eval_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_eval_duration: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval_duration: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OllamaTags {
    pub models: Vec<OllamaModel>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OllamaModel {
    pub name: String,
    pub model: String,
    pub modified_at: String,
    pub size: u64,
    pub digest: String,
    pub details: OllamaModelDetails,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct OllamaModelDetails {
    pub parent_model: String,
    pub format: String,
    pub family: String,
    pub families: Vec<String>,
    pub parameter_size: String,
    pub quantization_level: String,
}
