use std::time::Duration;

use uuid::Uuid;
use wire::task::TaskWire;

use super::WAIT_CEILING_MS;
use super::WAIT_DEFAULT_MS;
use super::WAIT_FLOOR_MS;
use super::body::store_error;
use super::error::TaskHttpError;
use super::present::to_wire;
use crate::tasks::Tasks;

#[derive(Debug)]
pub(super) enum WaitHit {
    Task(TaskWire),
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

pub(super) async fn wait_until(
    tasks: &Tasks,
    id: Uuid,
    timeout: Duration,
) -> Result<WaitHit, TaskHttpError> {
    let _waiting = tasks.waits.enter(id);
    let deadline = tokio::time::Instant::now() + timeout;
    let mut receiver = tasks.watch.subscribe();
    loop {
        let task = tasks
            .store
            .load(id)
            .await
            .map_err(|error| store_error(&error))?;
        if task.status.terminal() {
            tasks.waits.claim(id);
            return Ok(WaitHit::Task(to_wire(&task)?));
        }
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => {
                let task = tasks.store.load(id).await.map_err(|error| store_error(&error))?;
                if task.status.terminal() {
                    tasks.waits.claim(id);
                    return Ok(WaitHit::Task(to_wire(&task)?));
                }
                return Ok(WaitHit::Deadline(to_wire(&task)?));
            }
            changed = receiver.changed() => {
                if changed.is_err() {
                    return Err(TaskHttpError::Failed("task watch closed".to_owned()));
                }
            }
        }
    }
}
