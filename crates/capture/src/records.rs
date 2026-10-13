use serde::Serialize;

use crate::payload::HookEvent;

pub const SOURCE: &str = "claude-code-hook";

#[derive(Debug, Serialize)]
pub struct PromptSettings {
    pub source: &'static str,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub last_event: HookEvent,
    pub last_ingest_at: String,
}

#[derive(Debug, Serialize)]
pub struct TranscriptSettings {
    pub source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub session_id: String,
    pub pr_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub last_event: HookEvent,
    pub last_ingest_at: String,
}

#[derive(Debug, Serialize)]
pub struct HookEventMetadata<'a> {
    pub source: &'a str,
    pub hook_event: &'a str,
    pub ingest_key: &'a str,
}

#[derive(Debug, Serialize)]
pub struct TranscriptMetadata<'a> {
    pub source: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_jsonl_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_jsonl_line: Option<usize>,
    pub uuid: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidechain: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook_event: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovered: Option<bool>,
}
