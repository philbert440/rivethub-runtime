use std::time::Duration;

use kernel::tasks::ApprovalError;
use uuid::Uuid;
use wire::task::TaskApprovalRequest;
use wire::task::TaskWire;

use super::TasksApi;
use super::WAIT_CEILING_MS;
use super::WAIT_DEFAULT_MS;
use super::WAIT_FLOOR_MS;
use super::body::store_error;
use super::error::TaskHttpError;
use super::present::to_wire;

#[derive(Debug)]
pub(super) enum WaitHit {
    Task(TaskWire),
    Approval(TaskApprovalRequest),
    Deadline(TaskWire),
}

pub(super) fn wait_limit(raw: Option<&str>) -> Result<Duration, TaskHttpError> {
    let millis = match raw {
        None => WAIT_DEFAULT_MS,
        Some(text) => match text.parse::<u64>() {
            Ok(value) => value.clamp(WAIT_FLOOR_MS, WAIT_CEILING_MS),
            Err(_) => {
                return Err(TaskHttpError::BadRequest("invalid timeoutMs".to_owned()));
            }
        },
    };
    Ok(Duration::from_millis(millis))
}

pub(super) fn wants_wait(raw: Option<&str>) -> bool {
    matches!(raw, Some("1" | "true"))
}

pub(super) async fn wait_until(
    api: &TasksApi,
    id: Uuid,
    timeout: Duration,
    on_approval: bool,
) -> Result<WaitHit, TaskHttpError> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut receiver = api.watch.subscribe();
    loop {
        if let Some(hit) = ready(api, id, on_approval).await? {
            return Ok(hit);
        }
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => {
                let task = load(api, id).await?;
                return Ok(WaitHit::Deadline(to_wire(&task, &[])?));
            }
            changed = receiver.changed() => {
                if changed.is_err() {
                    return Err(TaskHttpError::Failed("task watch closed".to_owned()));
                }
            }
        }
    }
}

async fn ready(
    api: &TasksApi,
    id: Uuid,
    on_approval: bool,
) -> Result<Option<WaitHit>, TaskHttpError> {
    let task = load(api, id).await?;
    if kernel::tasks::terminal(task.status) {
        return Ok(Some(WaitHit::Task(to_wire(&task, &[])?)));
    }
    if on_approval && let Some(approval) = first_approval(api, id).await? {
        return Ok(Some(WaitHit::Approval(approval)));
    }
    Ok(None)
}

async fn first_approval(
    api: &TasksApi,
    id: Uuid,
) -> Result<Option<TaskApprovalRequest>, TaskHttpError> {
    let Some(broker) = &api.approvals else {
        return Ok(None);
    };
    let pending = broker
        .pending(id)
        .await
        .map_err(|error| approval_failed(&error))?;
    Ok(pending.into_iter().next())
}

async fn load(api: &TasksApi, id: Uuid) -> Result<kernel::tasks::TaskRecord, TaskHttpError> {
    api.store
        .load(id)
        .await
        .map_err(|error| store_error(&error))
}

fn approval_failed(error: &ApprovalError) -> TaskHttpError {
    let message = error.to_string();
    match error {
        ApprovalError::Unknown => TaskHttpError::Failed(message),
    }
}
