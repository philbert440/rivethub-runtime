use kernel::memory::jobs::{MemoryJobError, MemoryJobStore};
use kernel::memory::{EmbedTable, EmbedTarget};
use kernel::queue::{FixedJob, Job, JobKeyMode, JobName, JobQueue};
use tokio_util::sync::CancellationToken;

use crate::{WorkerError, embed_key};

pub async fn handle_enqueue_unembedded(
    store: &dyn MemoryJobStore,
    queue: &dyn JobQueue,
    limit: u32,
    max_attempts: u32,
    cancel: &CancellationToken,
) -> Result<usize, WorkerError> {
    let mut enqueued = 0;
    for table in [
        EmbedTable::Messages,
        EmbedTable::Summaries,
        EmbedTable::WikiTopics,
        EmbedTable::MessageChunks,
    ] {
        let targets = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(WorkerError::Cancelled),
            result = store.unembedded(table, limit) => result,
        };
        match targets {
            Ok(targets) => enqueued += enqueue(queue, targets, max_attempts, cancel).await?,
            Err(MemoryJobError::MissingTable { source, .. }) => {
                tracing::warn!("[enqueue-unembedded] {table} sweep failed: {source}");
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(enqueued)
}

pub(crate) async fn enqueue(
    queue: &dyn JobQueue,
    targets: Vec<EmbedTarget>,
    max_attempts: u32,
    cancel: &CancellationToken,
) -> Result<usize, WorkerError> {
    let count = targets.len();
    for target in targets {
        let job = embed_sweep_job(&target, max_attempts)?;
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(WorkerError::Cancelled),
            result = queue.enqueue(job) => { result.map_err(WorkerError::Queue)?; }
        }
    }
    Ok(count)
}

pub fn embed_sweep_job(target: &EmbedTarget, max_attempts: u32) -> Result<Job, WorkerError> {
    Ok(Job {
        name: JobName::Fixed(FixedJob::EmbedTarget),
        payload: crate::embed::embed_payload(target)?,
        key: Some(embed_key(target)),
        key_mode: JobKeyMode::KeepRunAt,
        priority: 0,
        run_at: None,
        max_attempts: Some(i32::try_from(max_attempts).map_err(|_| WorkerError::Count)?),
    })
}

pub async fn handle_enqueue_unchunked(
    store: &dyn MemoryJobStore,
    queue: &dyn JobQueue,
    settings: crate::EmbedSettings,
    limit: u32,
    max_attempts: u32,
    cancel: &CancellationToken,
) -> Result<usize, WorkerError> {
    if !settings.chunks_enabled {
        return Ok(0);
    }
    let targets = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(WorkerError::Cancelled),
        result = store.unchunked(limit, settings.chars_per_chunk, settings.tool_result_cap) => result?,
    };
    enqueue(queue, targets, max_attempts, cancel).await
}

pub async fn handle_enqueue_stale_wiki(
    store: &dyn MemoryJobStore,
    queue: &dyn JobQueue,
    enabled: bool,
    limit: u32,
    cancel: &CancellationToken,
) -> Result<usize, WorkerError> {
    if !enabled {
        return Ok(0);
    }
    let jobs = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(WorkerError::Cancelled),
        result = store.stale_wiki(limit) => result?,
    };
    for job in &jobs {
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(WorkerError::Cancelled),
            result = queue.enqueue(crate::wiki_builder::extract_wiki_job(job)?) => { result.map_err(WorkerError::Queue)?; }
        }
    }
    Ok(jobs.len())
}
