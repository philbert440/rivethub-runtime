use chrono::{DateTime, Timelike, Utc};
use kernel::memory::schedule::CronEntry;
use kernel::queue::{Job, JobKeyMode, JobQueue, WorkerRole};
use tokio_util::sync::CancellationToken;
use wire::JsonObject;

use crate::WorkerError;

pub async fn enqueue_due(
    queue: &dyn JobQueue,
    entries: &[CronEntry],
    now: DateTime<Utc>,
    cancel: &CancellationToken,
) -> Result<(), WorkerError> {
    for entry in entries {
        if !now.minute().is_multiple_of(entry.every_minutes) {
            continue;
        }
        let job = Job {
            name: entry.task.clone(),
            payload: JsonObject::default(),
            key: None,
            key_mode: JobKeyMode::Replace,
            priority: 0,
            run_at: None,
            max_attempts: Some(i32::try_from(entry.max_attempts).map_err(|_| WorkerError::Count)?),
        };
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(()),
            result = queue.enqueue(job) => { result.map_err(WorkerError::Queue)?; }
        }
    }
    Ok(())
}

#[must_use]
pub fn claimed_entries(
    queue: &dyn JobQueue,
    roles: &[WorkerRole],
    wiki_enabled: bool,
) -> Vec<CronEntry> {
    roles
        .iter()
        .flat_map(|role| {
            let claimed = queue.task_names(*role);
            role.crontab(wiki_enabled)
                .into_iter()
                .filter(move |entry| claimed.contains(&entry.task))
        })
        .collect()
}

pub(crate) async fn run(
    queue: &dyn JobQueue,
    roles: &[WorkerRole],
    wiki_enabled: bool,
    cancel: &CancellationToken,
) -> Result<(), WorkerError> {
    let entries = claimed_entries(queue, roles, wiki_enabled);
    loop {
        let now = Utc::now();
        let millis = 60_000 - now.timestamp_millis().rem_euclid(60_000);
        let wait = std::time::Duration::from_millis(
            u64::try_from(millis).map_err(|_| WorkerError::Count)?,
        );
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(()),
            () = tokio::time::sleep(wait) => {}
        }
        enqueue_due(queue, &entries, Utc::now(), cancel).await?;
    }
}
