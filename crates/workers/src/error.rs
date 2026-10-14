use kernel::memory::EmbeddingStoreError;
use kernel::memory::SummaryStoreError;
use kernel::queue::JobQueueError;
use thiserror::Error;
use uuid::Uuid;

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
    Target(#[source] EmbeddingStoreError),
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
    #[error("job payload is not a summarizer job: {source}")]
    SummaryPayload {
        #[source]
        source: serde_json::Error,
    },
    #[error("the summary store failed: {0}")]
    Summary(#[source] SummaryStoreError),
    #[error("{message}")]
    Llm { message: String },
    #[error("conversation {id} is not in the summary store")]
    ConversationMissing { id: Uuid },
    #[error("a truncated leaf batch cannot shrink further")]
    LeafShrink,
    #[error("the model returned an empty summary")]
    EmptySummary,
    #[error("the model truncated the summary")]
    Truncated,
    #[error("summary settings are invalid: {message}")]
    SummarySettings { message: String },
    #[error("a summary count does not fit")]
    Count,
    #[error("the summary clock overflowed")]
    Clock,
}

impl WorkerError {
    pub(crate) fn queue(source: JobQueueError) -> Self {
        Self::Queue(source)
    }

    pub(crate) fn payload(source: serde_json::Error) -> Self {
        Self::Payload { source }
    }

    pub(crate) fn summary_payload(source: serde_json::Error) -> Self {
        Self::SummaryPayload { source }
    }
}

impl From<EmbeddingStoreError> for WorkerError {
    fn from(source: EmbeddingStoreError) -> Self {
        Self::Target(source)
    }
}

impl From<SummaryStoreError> for WorkerError {
    fn from(source: SummaryStoreError) -> Self {
        Self::Summary(source)
    }
}
