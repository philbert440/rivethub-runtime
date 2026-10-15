use kernel::queue::FixedJob;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::tasks::NewJob;
use kernel::tasks::TaskJobPayload;
use kernel::tasks::TaskRecord;

use super::TasksApi;
use super::body::store_error;
use super::error::TaskHttpError;

pub(super) fn new_job(record: &TaskRecord, local: &str) -> NewJob {
    NewJob {
        name: job_name(record.node_affinity.as_deref(), local),
        payload: TaskJobPayload {
            task_id: record.id.to_string(),
        },
        key: format!("task:{}", record.id),
        key_mode: JobKeyMode::Replace,
        priority: 0,
        max_attempts: 1,
    }
}

pub(super) async fn enqueue(api: &TasksApi, record: &TaskRecord) -> Result<(), TaskHttpError> {
    api.jobs
        .enqueue(&new_job(record, &api.node))
        .await
        .map_err(|error| store_error(&error))?;
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
