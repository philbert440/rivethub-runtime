use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;
use wire::JsonObject;

use crate::error::WorkerError;

const SUGGEST_PRIORITY: i16 = 6;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SuggestPayload {
    pub(crate) summary_id: Uuid,
    pub(crate) conversation_id: Uuid,
}

pub fn suggest_tags_job(summary_id: Uuid, conversation_id: Uuid) -> Result<Job, WorkerError> {
    let payload = SuggestPayload {
        summary_id,
        conversation_id,
    };
    Ok(Job {
        name: JobName::Fixed(FixedJob::SuggestTags),
        payload: JsonObject::from_serialize(&payload).map_err(WorkerError::suggest_payload)?,
        key: Some(format!("tags-{summary_id}")),
        key_mode: JobKeyMode::Replace,
        priority: SUGGEST_PRIORITY,
        run_at: None,
    })
}

pub(crate) fn read_payload(payload: &JsonObject) -> Result<SuggestPayload, WorkerError> {
    let value = serde_json::to_value(payload).map_err(WorkerError::suggest_payload)?;
    serde_json::from_value(value).map_err(WorkerError::suggest_payload)
}
