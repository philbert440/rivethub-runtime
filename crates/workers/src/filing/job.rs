use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;
use wire::JsonObject;

use crate::error::WorkerError;

const FILE_PRIORITY: i16 = 6;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FilePayload {
    pub(crate) conversation_id: Uuid,
}

pub fn file_conversation_job(conversation_id: Uuid) -> Result<Job, WorkerError> {
    Ok(Job {
        name: JobName::Fixed(FixedJob::FileConversation),
        payload: JsonObject::from_serialize(&FilePayload { conversation_id })
            .map_err(WorkerError::file_payload)?,
        key: Some(format!("file-{conversation_id}")),
        key_mode: JobKeyMode::Replace,
        priority: FILE_PRIORITY,
        run_at: None,
        max_attempts: None,
    })
}

pub(crate) fn read_payload(payload: &JsonObject) -> Result<FilePayload, WorkerError> {
    let value = serde_json::to_value(payload).map_err(WorkerError::file_payload)?;
    serde_json::from_value(value).map_err(WorkerError::file_payload)
}
