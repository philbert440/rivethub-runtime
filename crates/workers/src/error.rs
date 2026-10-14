use kernel::queue::JobQueueError;
use thiserror::Error;

use crate::role::ProcessRole;

pub(crate) const NULL_EMBED_ERROR: &str = "Embedding returned null";
pub(crate) const PERMANENT_NULL_EMBED_ERROR: &str = "Embedding returned null (permanent)";

#[derive(Debug, Error)]
pub enum WorkerError {
    #[error("unknown worker role: {text}; expected {expected}")]
    UnknownRole { text: String, expected: String },
    #[error("{role} has no handler in this build")]
    RoleUnavailable { role: ProcessRole },
    #[error("worker concurrency must be at least 1")]
    Concurrency,
    #[error("embedding width must be at least 1")]
    Width,
    #[error("chunk window must be at least 1")]
    ChunkWindow,
    #[error("job payload is not an embed target: {source}")]
    Payload {
        #[source]
        source: serde_json::Error,
    },
    #[error("the job queue failed: {0}")]
    Queue(#[source] JobQueueError),
    #[error("the embed target store failed: {0}")]
    Target(String),
    #[error("embedding the model failed: {0}")]
    Embed(String),
    #[error("embedding timed out")]
    EmbedTimeout,
    #[error("Embedding returned null")]
    NullEmbedding,
    #[error("no handler for job {name}")]
    Unhandled { name: String },
    #[error("embed text is too long to index")]
    TextTooLong,
    #[error("the worker was cancelled")]
    Cancelled,
    #[error("a worker task ended: {0}")]
    TaskEnded(String),
    #[error("the worker semaphore is closed")]
    Closed,
}

impl WorkerError {
    pub(crate) fn queue(source: JobQueueError) -> Self {
        Self::Queue(source)
    }

    pub(crate) fn payload(source: serde_json::Error) -> Self {
        Self::Payload { source }
    }
}
