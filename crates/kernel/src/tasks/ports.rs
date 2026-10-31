use std::fmt::Debug;
use std::time::Duration;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use tokio::sync::broadcast;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::domain::Finish;
use super::domain::Harness;
use super::domain::Task;
use super::domain::TaskStatus;
use crate::BoxError;
use crate::queue::JobQueueError;

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("database connection failed")]
    Unavailable(#[source] BoxError),
    #[error("database operation failed: {operation}")]
    Operation {
        operation: &'static str,
        #[source]
        source: BoxError,
    },
    #[error("graphile worker queue is absent")]
    QueueAbsent,
    #[error("{context} failed ({primary}); cleanup also failed ({cleanup})")]
    Cleanup {
        context: &'static str,
        primary: String,
        cleanup: String,
    },
    #[error("job payload is not a task job")]
    Payload(#[source] BoxError),
    #[error("stored task field is invalid: {0}")]
    Stored(String),
    #[error("task {id} is missing")]
    Missing { id: Uuid },
    #[error("job {id} is missing")]
    MissingJob { id: i64 },
}

impl From<JobQueueError> for TaskError {
    fn from(value: JobQueueError) -> Self {
        match value {
            JobQueueError::Unavailable(source) => Self::Unavailable(source),
            JobQueueError::Claim { message, source } => Self::Operation {
                operation: "claim",
                source: Box::new(ClaimFailure { message, source }),
            },
            JobQueueError::Absent => Self::QueueAbsent,
            JobQueueError::Operation { operation, source } => Self::Operation { operation, source },
            JobQueueError::Missing { id } => Self::MissingJob { id: i64::from(id) },
            JobQueueError::Payload(source) => Self::Payload(source),
            JobQueueError::Stored(message) => Self::Stored(message),
        }
    }
}

#[derive(Debug, Error)]
#[error("{message}")]
struct ClaimFailure {
    message: String,
    #[source]
    source: BoxError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskJobPayload {
    pub task_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskFilter {
    pub status: Option<TaskStatus>,
    pub agent: Option<String>,
    pub limit: Option<u32>,
}

#[async_trait]
pub trait TaskStore: Debug + Send + Sync {
    async fn insert(&self, task: &Task) -> Result<(), TaskError>;

    async fn load(&self, id: Uuid) -> Result<Task, TaskError>;

    async fn list(&self, filter: &TaskFilter) -> Result<Vec<Task>, TaskError>;

    async fn by_child(&self, session: &str) -> Result<Option<Task>, TaskError>;

    async fn begin(&self, id: Uuid, node: &str, now: DateTime<Utc>) -> Result<bool, TaskError>;

    async fn attach(&self, id: Uuid, child: &str, harness: Harness) -> Result<(), TaskError>;

    async fn flip(&self, id: Uuid, status: TaskStatus) -> Result<bool, TaskError>;

    async fn alive(&self, ids: &[Uuid], now: DateTime<Utc>) -> Result<(), TaskError>;

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, TaskError>;

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, TaskError>;

    async fn orphans(&self, node: &str, now: DateTime<Utc>) -> Result<Vec<Uuid>, TaskError>;

    async fn reap(
        &self,
        stale_before: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, TaskError>;

    async fn expire(&self, now: DateTime<Utc>) -> Result<Vec<Uuid>, TaskError>;
}

pub trait TaskWatch: Debug + Send + Sync {
    fn subscribe(&self) -> watch::Receiver<u64>;

    fn changed(&self) -> broadcast::Receiver<Uuid>;
}

#[derive(Debug, Error)]
pub enum DelegationError {
    #[error("operation cancelled")]
    Cancelled,
    #[error("{0}")]
    Rejected(String),
    #[error(transparent)]
    Failed(BoxError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDirectory {
    pub presets: Vec<PresetAgent>,
    pub runtime: Vec<RuntimeAgent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetAgent {
    pub id: String,
    pub name: String,
    pub node: Option<String>,
    pub harness: Option<Harness>,
    pub directory: Option<String>,
    pub local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAgent {
    pub id: String,
    pub nodes: Vec<AgentNode>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentNode {
    pub id: String,
    pub last_seen: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegateRequest {
    pub target: String,
    pub prompt: String,
    pub criteria: Vec<String>,
    pub model: Option<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegateOutcome {
    pub id: Uuid,
    pub status: TaskStatus,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub child: Option<String>,
    pub node: String,
    pub tokens: Option<u64>,
    pub timed_out: bool,
}

#[async_trait]
pub trait Delegation: Send + Sync {
    async fn agents(&self) -> Result<AgentDirectory, DelegationError>;

    async fn delegate(
        &self,
        request: DelegateRequest,
        cancel: CancellationToken,
    ) -> Result<DelegateOutcome, DelegationError>;
}
