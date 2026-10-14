use chrono::DateTime;
use serde::Deserialize;
use serde::Deserializer;
use serde_with::DeserializeAs;
use serde_with::serde_as;
use thiserror::Error;
use wire::JsonObject;
use wire::capture::CaptureBatchRequest;
use wire::capture::CaptureMessage;

#[derive(Debug, Error)]
pub enum BatchError {
    #[error("invalid JSON")]
    Json(#[source] serde_json::Error),
    #[error("invalid capture batch: {0}")]
    Invalid(&'static str),
}

struct NotNull;

impl<'de, T: Deserialize<'de>> DeserializeAs<'de, Option<T>> for NotNull {
    fn deserialize_as<D: Deserializer<'de>>(deserializer: D) -> Result<Option<T>, D::Error> {
        T::deserialize(deserializer).map(Some)
    }
}

#[serde_as]
#[derive(Deserialize)]
struct BatchBody {
    session_key: String,
    agent: String,
    #[serde_as(as = "NotNull")]
    #[serde(default)]
    channel: Option<String>,
    #[serde_as(as = "NotNull")]
    #[serde(default)]
    title: Option<String>,
    #[serde_as(as = "NotNull")]
    #[serde(default)]
    settings: Option<JsonObject>,
    #[serde_as(as = "NotNull")]
    #[serde(default)]
    task_id: Option<String>,
    #[serde_as(as = "NotNull")]
    #[serde(default)]
    finalize: Option<bool>,
    #[serde_as(as = "NotNull")]
    #[serde(default)]
    created_at: Option<String>,
    #[serde_as(as = "NotNull")]
    #[serde(default)]
    updated_at: Option<String>,
    messages: Vec<CaptureMessage>,
}

impl From<BatchBody> for CaptureBatchRequest {
    fn from(body: BatchBody) -> Self {
        Self {
            session_key: body.session_key,
            agent: body.agent,
            channel: body.channel,
            title: body.title,
            settings: body.settings,
            task_id: body.task_id,
            finalize: body.finalize,
            created_at: body.created_at,
            updated_at: body.updated_at,
            messages: body.messages,
        }
    }
}

pub fn parse_batch(body: &str) -> Result<CaptureBatchRequest, BatchError> {
    let batch: CaptureBatchRequest = serde_json::from_str::<BatchBody>(body)
        .map_err(BatchError::Json)?
        .into();
    check_semantics(&batch)?;
    Ok(batch)
}

fn check_semantics(batch: &CaptureBatchRequest) -> Result<(), BatchError> {
    if batch.session_key.trim().is_empty() {
        return Err(BatchError::Invalid("session_key is blank"));
    }
    if batch.agent.trim().is_empty() {
        return Err(BatchError::Invalid("agent is blank"));
    }
    let timestamps = [&batch.created_at, &batch.updated_at]
        .into_iter()
        .chain(batch.messages.iter().map(|message| &message.created_at))
        .flatten();
    for at in timestamps {
        DateTime::parse_from_rfc3339(at)
            .map_err(|_| BatchError::Invalid("timestamps are ISO datetimes with an offset"))?;
    }
    if batch
        .messages
        .iter()
        .any(|message| message.event_id.is_empty())
    {
        return Err(BatchError::Invalid("event_id is empty"));
    }
    Ok(())
}
