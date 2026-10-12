use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use strum::Display;
use strum::EnumString;
use thiserror::Error;
use wire::JsonObject;

#[derive(Debug, Error)]
pub enum JobQueueError {
    #[error("job queue is unavailable: {0}")]
    Unavailable(String),
    #[error("job claim failed: {0}")]
    Claim(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKeyMode {
    KeepRunAt,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Display, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum JobName {
    EmbedTarget,
    EnqueueUnembedded,
    EnqueueUnchunked,
    CompactConversation,
    SynthesizeToolCall,
    SuggestTags,
    EnqueueIdle,
    ExtractWiki,
    EnqueueWikiBackfill,
    ConsolidateWiki,
    RecompileWiki,
    EnqueueStaleWiki,
    EnqueueStaleCompaction,
    ReapDeadJobs,
    RunTask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkerRole {
    Embedder,
    Summarizer,
    Categorizer,
    WikiBuilder,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub name: JobName,
    pub payload: JsonObject,
    pub key: Option<String>,
    pub key_mode: JobKeyMode,
    pub priority: i64,
    pub run_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedJob {
    pub id: i64,
    pub name: JobName,
    pub payload: JsonObject,
    pub attempt: u32,
}

#[async_trait]
pub trait JobQueue: Send + Sync {
    async fn enqueue(&self, job: Job) -> Result<(), JobQueueError>;

    async fn claim(&self, role: WorkerRole) -> Result<Option<ClaimedJob>, JobQueueError>;

    async fn complete(&self, id: i64) -> Result<(), JobQueueError>;

    async fn fail(&self, id: i64, error: &str) -> Result<(), JobQueueError>;
}
