use crate::BoxError;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use std::str::FromStr;
use thiserror::Error;
use wire::JsonObject;

#[derive(Debug, Error)]
pub enum JobQueueError {
    #[error("job queue is unavailable")]
    Unavailable(#[source] BoxError),
    #[error("job claim failed: {message}")]
    Claim {
        message: String,
        #[source]
        source: BoxError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(i64);

impl JobId {
    #[must_use]
    pub fn new(value: i64) -> Self {
        Self(value)
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<JobId> for i64 {
    fn from(value: JobId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum JobNameError {
    #[error("unknown job name: {0}")]
    Unknown(String),
    #[error("heartbeat job name needs a numeric index after the last dash: {0}")]
    HeartbeatIndex(String),
    #[error("run-task job name has an empty node")]
    EmptyNode,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    strum::Display,
    strum::EnumString,
    strum::EnumIter,
)]
#[strum(serialize_all = "kebab-case")]
pub enum FixedJob {
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

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum JobName {
    Fixed(FixedJob),
    Heartbeat { agent: String, index: u32 },
    RunTaskOn { node: String },
}

impl fmt::Display for JobName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fixed(fixed) => write!(f, "{fixed}"),
            Self::Heartbeat { agent, index } => write!(f, "heartbeat-{agent}-{index}"),
            Self::RunTaskOn { node } => write!(f, "run-task:{node}"),
        }
    }
}

impl FromStr for JobName {
    type Err = JobNameError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(rest) = value.strip_prefix("heartbeat-") {
            let Some((agent, index)) = rest.rsplit_once('-') else {
                return Err(JobNameError::HeartbeatIndex(value.to_string()));
            };
            let index = index
                .parse::<u32>()
                .map_err(|_| JobNameError::HeartbeatIndex(value.to_string()))?;
            return Ok(Self::Heartbeat {
                agent: agent.to_string(),
                index,
            });
        }
        if let Some(node) = value.strip_prefix("run-task:") {
            if node.is_empty() {
                return Err(JobNameError::EmptyNode);
            }
            return Ok(Self::RunTaskOn {
                node: node.to_string(),
            });
        }
        FixedJob::from_str(value)
            .map(Self::Fixed)
            .map_err(|_| JobNameError::Unknown(value.to_string()))
    }
}

impl TryFrom<String> for JobName {
    type Error = JobNameError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_str(&value)
    }
}

impl From<JobName> for String {
    fn from(value: JobName) -> Self {
        value.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKeyMode {
    KeepRunAt,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub id: JobId,
    pub name: JobName,
    pub payload: JsonObject,
    pub attempt: u32,
}

#[async_trait]
pub trait JobQueue: Send + Sync {
    async fn enqueue(&self, job: Job) -> Result<(), JobQueueError>;

    async fn claim(&self, role: WorkerRole) -> Result<Option<ClaimedJob>, JobQueueError>;

    async fn complete(&self, id: JobId) -> Result<(), JobQueueError>;

    async fn fail(&self, id: JobId, error: &str) -> Result<(), JobQueueError>;
}
