use std::sync::LazyLock;

use axum::extract::FromRequest;
use axum::extract::Request;
use chrono::DateTime;
use chrono::Utc;
use regex::Regex;
use serde::Serialize;
use wire::capture::CaptureBatchRequest;
use wire::json::JsonObject;
use wire::memory::MemoryEntry;
use wire::message::Role;

use crate::error::ApiError;
use crate::error::invalid_json;
use crate::extract::limited_bytes;

const CAPTURE_BODY_LIMIT: usize = 1024 * 1024;

const DATETIME_PATTERN: &str = concat!(
    r"^(?:(?:\d\d[2468][048]|\d\d[13579][26]|\d\d0[48]|[02468][048]00|[13579][26]00)-02-29",
    r"|\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\d|30)",
    r"|(?:02)-(?:0[1-9]|1\d|2[0-8])))T(?:(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?",
    r"(?:Z|([+-](?:[01]\d|2[0-3]):[0-5]\d)))$"
);

#[expect(
    clippy::expect_used,
    reason = "constant pattern; the unit test compiles it"
)]
static DATETIME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(DATETIME_PATTERN).expect("datetime pattern compiles"));

static DATETIME_PATTERN_DELIMITED: LazyLock<String> =
    LazyLock::new(|| format!("/{DATETIME_PATTERN}/"));

#[derive(Debug)]
pub(crate) struct ValidatedBatch {
    pub(crate) session_key: String,
    agent: String,
    channel: Option<String>,
    messages: Vec<ValidatedMessage>,
}

#[derive(Debug)]
pub(crate) struct ValidatedMessage {
    event_id: String,
    role: Role,
    content: String,
    tool_name: Option<String>,
    tool_args: Option<JsonObject>,
    tool_result: Option<String>,
    metadata: Option<JsonObject>,
    created_at: Option<DateTime<Utc>>,
}

impl ValidatedBatch {
    pub(crate) fn entries(&self) -> Vec<MemoryEntry> {
        self.messages
            .iter()
            .map(|message| MemoryEntry {
                id: Some(message.event_id.clone()),
                session_id: self.session_key.clone(),
                agent: self.agent.clone(),
                channel: self.channel.clone().unwrap_or_default(),
                role: message.role,
                content: message.content.clone(),
                tool_name: message.tool_name.clone(),
                tool_args: message.tool_args.clone(),
                tool_result: message.tool_result.clone(),
                metadata: message.metadata.clone(),
                created_at: message.created_at,
            })
            .collect()
    }
}

impl<S> FromRequest<S> for ValidatedBatch
where
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = limited_bytes(request, state).await?;
        let sanitized = wire::json::replace_lone_surrogates(&bytes);
        let batch: CaptureBatchRequest =
            serde_json::from_slice(&sanitized).map_err(|_| invalid_json())?;
        validate(batch)
    }
}

pub(crate) fn validate(batch: CaptureBatchRequest) -> Result<ValidatedBatch, ApiError> {
    let mut issues = Vec::new();
    non_empty(&mut issues, &batch.session_key, "session_key");
    non_empty(&mut issues, &batch.agent, "agent");
    datetime_issue(&mut issues, batch.created_at.as_deref(), "created_at");
    datetime_issue(&mut issues, batch.updated_at.as_deref(), "updated_at");
    let mut messages = Vec::with_capacity(batch.messages.len());
    for (index, message) in batch.messages.into_iter().enumerate() {
        if message.event_id.is_empty() {
            issues.push(too_small(message_path(index, "event_id")));
        }
        let created_at = message_timestamp(&mut issues, index, message.created_at);
        messages.push(ValidatedMessage {
            event_id: message.event_id,
            role: message.role,
            content: message.content,
            tool_name: message.tool_name,
            tool_args: message.tool_args,
            tool_result: message.tool_result,
            metadata: message.metadata,
            created_at,
        });
    }
    if issues.is_empty() {
        Ok(ValidatedBatch {
            session_key: batch.session_key,
            agent: batch.agent,
            channel: batch.channel,
            messages,
        })
    } else {
        Err(ApiError::BadRequest(issue_text(&issues)?))
    }
}

#[derive(Serialize)]
struct Issue {
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<&'static str>,
    code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    minimum: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inclusive: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: Option<&'static str>,
    path: Vec<PathSegment>,
    message: &'static str,
}

#[derive(Clone, Serialize)]
#[serde(untagged)]
enum PathSegment {
    Field(String),
    Index(usize),
}

fn too_small(path: Vec<PathSegment>) -> Issue {
    Issue {
        origin: Some("string"),
        code: "too_small",
        minimum: Some(1),
        inclusive: Some(true),
        format: None,
        pattern: None,
        path,
        message: "Too small: expected string to have >=1 characters",
    }
}

fn trimmed(path: Vec<PathSegment>) -> Issue {
    Issue {
        origin: None,
        code: "custom",
        minimum: None,
        inclusive: None,
        format: None,
        pattern: None,
        path,
        message: "Invalid input",
    }
}

fn invalid_datetime(path: Vec<PathSegment>) -> Issue {
    Issue {
        origin: Some("string"),
        code: "invalid_format",
        minimum: None,
        inclusive: None,
        format: Some("datetime"),
        pattern: Some(DATETIME_PATTERN_DELIMITED.as_str()),
        path,
        message: "Invalid ISO datetime",
    }
}

fn issue_text(issues: &[Issue]) -> Result<String, ApiError> {
    serde_json::to_string_pretty(issues).map_err(ApiError::Serialize)
}

#[must_use]
pub fn body_limit() -> usize {
    CAPTURE_BODY_LIMIT
}

fn non_empty(issues: &mut Vec<Issue>, value: &str, field: &str) {
    let path = vec![PathSegment::Field(field.to_string())];
    if value.is_empty() {
        issues.push(too_small(path.clone()));
    }
    if value.trim().is_empty() {
        issues.push(trimmed(path));
    }
}

fn datetime_issue(issues: &mut Vec<Issue>, value: Option<&str>, field: &str) {
    if let Some(value) = value
        && !iso_datetime_with_offset(value)
    {
        issues.push(invalid_datetime(vec![PathSegment::Field(
            field.to_string(),
        )]));
    }
}

fn message_timestamp(
    issues: &mut Vec<Issue>,
    index: usize,
    raw: Option<String>,
) -> Option<DateTime<Utc>> {
    let raw = raw?;
    if !iso_datetime_with_offset(&raw) {
        issues.push(invalid_datetime(message_path(index, "created_at")));
        return None;
    }
    let created_at = DateTime::parse_from_rfc3339(&raw)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc));
    if created_at.is_none() {
        issues.push(invalid_datetime(message_path(index, "created_at")));
    }
    created_at
}

fn message_path(index: usize, field: &str) -> Vec<PathSegment> {
    vec![
        PathSegment::Field("messages".to_string()),
        PathSegment::Index(index),
        PathSegment::Field(field.to_string()),
    ]
}

#[must_use]
pub fn iso_datetime_with_offset(value: &str) -> bool {
    DATETIME.is_match(value)
}

#[cfg(test)]
mod validated_batch_tests {
    use wire::capture::CaptureBatchRequest;
    use wire::capture::CaptureMessage;
    use wire::message::Role;

    use super::validate;

    #[test]
    fn validated_batches_carry_typed_timestamps() {
        let batch = CaptureBatchRequest {
            session_key: "s".to_owned(),
            agent: "a".to_owned(),
            channel: None,
            title: None,
            settings: None,
            task_id: None,
            finalize: None,
            created_at: None,
            updated_at: None,
            messages: vec![CaptureMessage {
                event_id: "e1".to_owned(),
                role: Role::User,
                content: "hi".to_owned(),
                tool_name: None,
                tool_args: None,
                tool_result: None,
                metadata: None,
                created_at: Some("2026-10-10T12:00:00+02:00".to_owned()),
            }],
        };
        let validated = validate(batch).expect("valid batch");
        let entries = validated.entries();
        let created_at = entries.first().and_then(|entry| entry.created_at);
        assert_eq!(
            created_at,
            Some(
                chrono::DateTime::parse_from_rfc3339("2026-10-10T10:00:00Z")
                    .expect("instant")
                    .with_timezone(&chrono::Utc)
            )
        );
    }

    #[test]
    fn datetime_pattern_compiles_and_matches_the_recorded_examples() {
        assert!(super::DATETIME.is_match("2026-10-10T12:00:00Z"));
        assert!(super::DATETIME.is_match("2026-10-10T12:00:00.000+02:00"));
        assert!(!super::DATETIME.is_match("2026-13-01T00:00:00Z"));
        assert!(!super::DATETIME.is_match("yesterday"));
    }
}
