use serde::Serialize;
use values::JsonObject;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    pub attachments: Option<Vec<TranscriptAttachment>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub complete: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub compact: bool,
    pub started_at_ms: Option<i64>,
    pub ended_at_ms: Option<i64>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptAttachment {
    pub media_type: String,
    pub name: Option<String>,
    pub uri: Option<String>,
    pub upload_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        duptype,
        reason = "KERNELWIRE: the kernel owns a turn's last block kind; wire::gateway::sessions::TranscriptBlock is the contract's copy, translated by the gateway"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptBlock {
    Thinking,
    Text,
    ToolUse,
    ToolResult,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        duptype,
        reason = "KERNELWIRE: the kernel owns a transcript tool's status; wire::gateway::sessions::TranscriptToolStatus is the contract's copy, translated by the gateway"
    )
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TranscriptToolStatus {
    Running,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionMessageRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarnessToolOutput {
    pub text: String,
    pub is_error: bool,
    pub truncated: bool,
}
