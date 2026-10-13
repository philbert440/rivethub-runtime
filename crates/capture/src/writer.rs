use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use serde::Serialize;
use thiserror::Error;
use wire::JsonObject;
use wire::capture::CaptureBatchRequest;
use wire::capture::CaptureMessage;
use wire::capture::CaptureResult;

use crate::json;
use crate::json::JsJson;
use crate::json::JsonError;
use crate::redaction::Redaction;
use crate::redaction::RedactionError;
use crate::spool::CaptureSpool;

const CONTENT_LIMIT: usize = 16_000;
pub const DEFAULT_CHUNK_BYTES: usize = 768 * 1024;
const DEFAULT_REPLAY_MAX: usize = 50;

fn metadata_keep() -> &'static [&'static str] {
    &[
        "event_id",
        "session_jsonl_path",
        "session_jsonl_line",
        "session_sqlite_path",
        "session_sqlite_part_id",
        "truncated",
        "ordinal",
        "native_event_id",
        "source",
        "metadata_elided",
        "full_metadata_bytes",
    ]
}

#[derive(Debug, Error)]
pub enum PostError {
    #[error("capture HTTP {status}")]
    Client { status: u16 },
    #[error("capture HTTP {status}")]
    Server { status: u16 },
    #[error("capture request failed: {message}")]
    Network { message: String },
}

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("chunk exceeds maxChunkBytes after elision")]
    ChunkTooLarge,
    #[error(transparent)]
    Spool(#[from] crate::spool::SpoolError),
    #[error(transparent)]
    Post(#[from] PostError),
    #[error(transparent)]
    Replay(#[from] crate::spool::ReplayError),
    #[error(transparent)]
    Redaction(#[from] RedactionError),
    #[error(transparent)]
    Json(#[from] JsonError),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Totals {
    pub inserted: u64,
    pub skipped: u64,
    pub conversation_id: String,
}

impl Totals {
    fn add(&mut self, result: &CaptureResult) {
        self.inserted += result.inserted;
        self.skipped += result.skipped;
        if !result.conversation_id.is_empty() {
            self.conversation_id.clone_from(&result.conversation_id);
        }
    }
}

#[async_trait]
pub trait CaptureTransport: Send + Sync {
    async fn post(&self, body: String) -> Result<CaptureResult, PostError>;
}

#[derive(Debug)]
pub struct HttpCaptureTransport {
    endpoint: String,
    client: reqwest::Client,
}

impl HttpCaptureTransport {
    #[must_use]
    pub fn new(endpoint: String, client: reqwest::Client) -> Self {
        Self { endpoint, client }
    }
}

#[async_trait]
impl CaptureTransport for HttpCaptureTransport {
    async fn post(&self, body: String) -> Result<CaptureResult, PostError> {
        let url = format!("{}/api/capture", self.endpoint.trim_end_matches('/'));
        let response = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|error| PostError::Network {
                message: error.to_string(),
            })?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            if (400..500).contains(&status) {
                return Err(PostError::Client { status });
            }
            return Err(PostError::Server { status });
        }
        response
            .json::<CaptureResult>()
            .await
            .map_err(|error| PostError::Network {
                message: error.to_string(),
            })
    }
}

#[derive(Debug)]
pub struct CaptureWriter<T: CaptureTransport> {
    transport: T,
    spool: CaptureSpool,
    max_chunk_bytes: usize,
    redaction: Option<Redaction>,
}

impl<T: CaptureTransport> CaptureWriter<T> {
    #[must_use]
    pub fn new(
        transport: T,
        spool: CaptureSpool,
        max_chunk_bytes: usize,
        redaction: Option<Redaction>,
    ) -> Self {
        Self {
            transport,
            spool,
            max_chunk_bytes: if max_chunk_bytes == 0 {
                DEFAULT_CHUNK_BYTES
            } else {
                max_chunk_bytes
            },
            redaction,
        }
    }

    pub async fn write(
        &self,
        batch: &CaptureBatchRequest,
        now: DateTime<Utc>,
    ) -> Result<Totals, WriteError> {
        self.spool
            .replay(DEFAULT_REPLAY_MAX, |body| self.transport.post(body))
            .await?;
        let mut prepared = batch.clone();
        if let Some(redaction) = &self.redaction {
            let mut spans = 0;
            for message in &mut prepared.messages {
                spans += redaction.redact_message(message)?;
            }
            if spans > 0 {
                tracing::info!("redacted {spans} spans");
            }
        }
        cap_batch(&mut prepared)?;
        let chunks = split_chunks(&prepared, self.max_chunk_bytes)?;
        let mut totals = Totals::default();
        for chunk in chunks {
            let body = body_text(&chunk)?;
            match self.transport.post(body.clone()).await {
                Ok(result) => totals.add(&result),
                Err(error @ PostError::Client { .. }) => return Err(error.into()),
                Err(error) => {
                    self.spool.spool(&body, now)?;
                    return Err(error.into());
                }
            }
        }
        Ok(totals)
    }
}

fn body_text(batch: &CaptureBatchRequest) -> Result<String, WriteError> {
    serde_json::to_string(batch)
        .map_err(JsonError::Encode)
        .map_err(WriteError::from)
}

fn cap_batch(batch: &mut CaptureBatchRequest) -> Result<(), WriteError> {
    for message in &mut batch.messages {
        cap_message(message)?;
    }
    Ok(())
}

fn cap_message(message: &mut CaptureMessage) -> Result<(), WriteError> {
    if utf16_len(&message.content) > CONTENT_LIMIT {
        let full = utf16_len(&message.content);
        message.content = utf16_cut(&message.content, CONTENT_LIMIT);
        with_metadata(&mut message.metadata, |members| {
            json::set_member(members, "truncated", JsJson::Bool(true));
            json::set_member(members, "full_content_length", json::number(full));
        })?;
    }
    if let Some(result) = &message.tool_result
        && utf16_len(result) > CONTENT_LIMIT
    {
        let full = utf16_len(result);
        message.tool_result = Some(utf16_cut(result, CONTENT_LIMIT));
        with_metadata(&mut message.metadata, |members| {
            json::set_member(members, "truncated", JsJson::Bool(true));
            json::set_member(members, "full_tool_result_length", json::number(full));
        })?;
    }
    Ok(())
}

fn with_metadata(
    metadata: &mut Option<JsonObject>,
    edit: impl FnOnce(&mut Vec<(String, JsJson)>),
) -> Result<(), JsonError> {
    let mut members = match metadata {
        Some(existing) => json::members_of(existing)?,
        None => Vec::new(),
    };
    edit(&mut members);
    *metadata = Some(json::object_of(members)?);
    Ok(())
}

#[derive(Serialize)]
struct ElidedBlob {
    #[serde(rename = "_elided")]
    elided: bool,
    bytes: usize,
}

fn elided_blob(bytes: usize) -> Result<JsonObject, JsonError> {
    json::encode(&ElidedBlob {
        elided: true,
        bytes,
    })
}

#[must_use]
pub fn utf16_cut(text: &str, limit: usize) -> String {
    let mut units = 0;
    let mut cut = text.len();
    for (index, character) in text.char_indices() {
        if units + character.len_utf16() > limit {
            cut = index;
            break;
        }
        units += character.len_utf16();
        cut = index + character.len_utf8();
    }
    text[..cut].to_owned()
}

#[must_use]
pub fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

fn is_full_length_key(key: &str) -> bool {
    key.starts_with("full_") && key.ends_with("_length")
}

fn filter_metadata(metadata: &JsonObject, original_bytes: usize) -> Result<JsonObject, JsonError> {
    let mut members = json::members_of(metadata)?;
    members.retain(|(key, _)| metadata_keep().contains(&key.as_str()) || is_full_length_key(key));
    json::set_member(&mut members, "metadata_elided", JsJson::Bool(true));
    json::set_member(
        &mut members,
        "full_metadata_bytes",
        json::number(original_bytes),
    );
    json::object_of(members)
}

fn split_chunks(
    batch: &CaptureBatchRequest,
    max_chunk_bytes: usize,
) -> Result<Vec<CaptureBatchRequest>, WriteError> {
    let mut base = batch.clone();
    base.messages = Vec::new();
    base.finalize = None;
    if let Some(settings) = &base.settings {
        let bytes = json::object_bytes(settings)?;
        if bytes > max_chunk_bytes {
            base.settings = Some(elided_blob(bytes)?);
        }
    }
    let mut chunks: Vec<CaptureBatchRequest> = Vec::new();
    let mut current = base.clone();
    for message in &batch.messages {
        let mut candidate = current.clone();
        candidate.messages.push(message.clone());
        if body_bytes(&candidate)? <= max_chunk_bytes {
            current = candidate;
            continue;
        }
        if !current.messages.is_empty() {
            chunks.push(std::mem::replace(&mut current, base.clone()));
        }
        current = elided_chunk(&base, message, max_chunk_bytes)?;
    }
    chunks.push(current);
    if let Some(last) = chunks.last_mut() {
        last.finalize = batch.finalize;
    }
    Ok(chunks)
}

fn elided_chunk(
    base: &CaptureBatchRequest,
    original: &CaptureMessage,
    max_chunk_bytes: usize,
) -> Result<CaptureBatchRequest, WriteError> {
    let mut reduced = original.clone();
    if let Some(alone) = chunk_with(base, &reduced, max_chunk_bytes)? {
        return Ok(alone);
    }
    if let Some(arguments) = &original.tool_args {
        let bytes = json::object_bytes(arguments)?;
        let length = json::object_chars(arguments)?;
        reduced.tool_args = Some(elided_blob(bytes)?);
        with_metadata(&mut reduced.metadata, |members| {
            json::set_member(members, "full_tool_args_length", json::number(length));
        })?;
        if let Some(alone) = chunk_with(base, &reduced, max_chunk_bytes)? {
            return Ok(alone);
        }
    }
    if let Some(existing) = &original.metadata {
        let bytes = json::object_bytes(existing)?;
        reduced.metadata = Some(filter_metadata(existing, bytes)?);
        if let Some(alone) = chunk_with(base, &reduced, max_chunk_bytes)? {
            return Ok(alone);
        }
    }
    Err(WriteError::ChunkTooLarge)
}

fn chunk_with(
    base: &CaptureBatchRequest,
    message: &CaptureMessage,
    max_chunk_bytes: usize,
) -> Result<Option<CaptureBatchRequest>, WriteError> {
    let mut alone = base.clone();
    alone.messages.push(message.clone());
    Ok((body_bytes(&alone)? <= max_chunk_bytes).then_some(alone))
}

fn body_bytes(batch: &CaptureBatchRequest) -> Result<usize, WriteError> {
    Ok(body_text(batch)?.len())
}
