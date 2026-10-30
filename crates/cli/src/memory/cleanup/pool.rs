use std::future::Future;

use futures::StreamExt as _;
use serde::Serialize;
use workers::WorkerError;

use super::Settings;
use super::book::Book;
use crate::Error;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Tally {
    pub(super) done: usize,
    pub(super) failed: usize,
}

pub(super) async fn drive<J, R, W, Fut>(
    jobs: Vec<J>,
    settings: &Settings,
    book: &Book,
    file: &str,
    work: W,
) -> Result<(Vec<R>, Tally), Error>
where
    J: Clone,
    R: Serialize,
    W: Fn(J) -> Fut,
    Fut: Future<Output = Result<Vec<R>, WorkerError>>,
{
    let total = jobs.len();
    let work = &work;
    let attempts = settings.attempts;
    let mut running = futures::stream::iter(jobs)
        .map(|job| async move {
            let mut last = WorkerError::Cleanup {
                message: "no attempt was made".to_owned(),
            };
            for _ in 0..attempts {
                match work(job.clone()).await {
                    Ok(value) => return Ok(value),
                    Err(WorkerError::Cancelled) => return Err(WorkerError::Cancelled),
                    Err(error) => {
                        tracing::warn!("cleanup model call failed: {error}");
                        last = error;
                    }
                }
            }
            Err(last)
        })
        .buffer_unordered(settings.concurrency);
    let mut tally = Tally::default();
    let mut results = Vec::new();
    while let Some(outcome) = running.next().await {
        match outcome {
            Ok(values) => {
                book.append(file, &values).await?;
                results.extend(values);
                tally.done = tally.done.saturating_add(1);
            }
            Err(WorkerError::Cancelled) => return Err(Error::Cancelled),
            Err(error) => {
                book.note(&format!("{file}: a job gave up: {error}"))
                    .await?;
                tally.failed = tally.failed.saturating_add(1);
            }
        }
        book.note(&format!(
            "{file}: {} of {total} jobs done, {} failed",
            tally.done, tally.failed
        ))
        .await?;
    }
    Ok((results, tally))
}
