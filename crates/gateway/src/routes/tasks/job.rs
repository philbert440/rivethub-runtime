use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::tasks::TaskJobPayload;
use kernel::tasks::TaskRecord;
use wire::JsonObject;

use super::TasksApi;
use super::body::store_error;
use super::error::TaskHttpError;

pub(super) fn new_job(record: &TaskRecord, local: &str) -> Result<Job, TaskHttpError> {
    Ok(Job {
        name: job_name(record.node_affinity.as_deref(), local),
        payload: JsonObject::from_serialize(&TaskJobPayload {
            task_id: record.id.to_string(),
        })
        .map_err(|error| TaskHttpError::Failed(error.to_string()))?,
        key: Some(format!("task:{}", record.id)),
        key_mode: JobKeyMode::Replace,
        priority: 0,
        run_at: None,
        max_attempts: Some(1),
    })
}

pub(super) async fn enqueue(api: &TasksApi, record: &TaskRecord) -> Result<(), TaskHttpError> {
    api.jobs
        .enqueue(new_job(record, &api.node)?)
        .await
        .map_err(|error| store_error(&error.into()))?;
    Ok(())
}

fn job_name(affinity: Option<&str>, local: &str) -> JobName {
    match affinity {
        Some(node) if !node.is_empty() && node != local => JobName::RunTaskOn {
            node: node.to_owned(),
        },
        Some(_) | None => JobName::Fixed(FixedJob::RunTask),
    }
}
