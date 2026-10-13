use chrono::DateTime;
use thiserror::Error;
use wire::JsonObject;
use wire::capture::CaptureBatchRequest;

fn nullable_members() -> &'static [&'static str] {
    &[
        "channel",
        "title",
        "settings",
        "task_id",
        "finalize",
        "created_at",
        "updated_at",
        "tool_name",
        "tool_result",
        "metadata",
    ]
}

#[derive(Debug, Error)]
pub enum BatchError {
    #[error("invalid JSON")]
    Json(#[source] serde_json::Error),
    #[error("invalid capture batch: {0}")]
    Invalid(&'static str),
}

pub fn parse_batch(body: &str) -> Result<CaptureBatchRequest, BatchError> {
    let raw: JsonObject = serde_json::from_str(body).map_err(BatchError::Json)?;
    for member in nullable_members() {
        if matches!(raw.get(member), Some(value) if value.is_null()) {
            return Err(BatchError::Invalid("explicit null is not a value"));
        }
    }
    let mut deserializer = serde_json::Deserializer::from_str(body);
    let batch: CaptureBatchRequest = serde_path_to_error::deserialize(&mut deserializer)
        .map_err(|error| BatchError::Json(error.into_inner()))?;
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
    if let Some(at) = &batch.created_at {
        check_timestamp(at)?;
    }
    if let Some(at) = &batch.updated_at {
        check_timestamp(at)?;
    }
    for message in &batch.messages {
        if message.event_id.is_empty() {
            return Err(BatchError::Invalid("event_id is empty"));
        }
        if let Some(at) = &message.created_at {
            check_timestamp(at)?;
        }
    }
    Ok(())
}

fn check_timestamp(text: &str) -> Result<(), BatchError> {
    DateTime::parse_from_rfc3339(text)
        .map(|_| ())
        .map_err(|_| BatchError::Invalid("timestamps are ISO datetimes with an offset"))
}
