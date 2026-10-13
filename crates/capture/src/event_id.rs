use sha2::Digest;
use sha2::Sha256;

use crate::json;
use crate::json::JsJson;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display)]
#[strum(serialize_all = "lowercase")]
pub enum OccurrenceKind {
    Tool,
    Assistant,
}

#[must_use]
pub fn digest_text(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

#[must_use]
pub fn arguments_spelling(arguments: &JsJson) -> String {
    json::spelling(arguments)
}

#[must_use]
pub fn occurrence_digest(
    kind: OccurrenceKind,
    content: &str,
    tool_name: &str,
    arguments: Option<&JsJson>,
) -> String {
    let spelling = arguments.map_or_else(String::new, json::spelling);
    digest_text(&format!("{kind}\0{content}\0{tool_name}\0{spelling}"))
}

#[must_use]
pub fn occurrence_event_id(session_key: &str, digest: &str, occurrence: usize) -> String {
    format!("{session_key}:occ:{digest}:{occurrence}")
}
