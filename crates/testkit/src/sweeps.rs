use kernel::queue::ClaimSelector;
use kernel::queue::FixedJob;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::WorkerRole;

use crate::topics::Outcome;

pub async fn a_pending_sweep_is_queued_once(queue: &dyn JobQueue) -> Outcome {
    let idle = JobName::Fixed(FixedJob::EnqueueIdle);
    let entry = WorkerRole::Summarizer
        .crontab(false)
        .into_iter()
        .find(|entry| entry.task == idle)
        .ok_or("the summarizer schedules no idle sweep")?;
    queue.enqueue(entry.job()).await?;
    queue.enqueue(entry.job()).await?;
    let selector = ClaimSelector::matching(vec![idle]);
    let claimed = queue.claim(&selector).await?.ok_or("no sweep is pending")?;
    queue.complete(claimed.id).await?;
    assert_eq!(queue.claim(&selector).await?, None);
    Ok(())
}
