use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::task::AcceptanceCriterion;
use wire::task::ContextRef;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskResult;
use wire::task::TaskSpec;
use wire::task::TaskStatus;
use wire::task::TaskUsage;
use wire::task::VerifierResult;

use crate::BoxError;
use crate::queue::JobKeyMode;
use crate::queue::JobName;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    Run,
    Steer,
    Kill,
}

impl std::fmt::Display for ConflictAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Run => "run",
            Self::Steer => "steer",
            Self::Kill => "kill",
        })
    }
}

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
    #[error("task {id} is {status:?} and cannot {action}")]
    Conflict {
        id: Uuid,
        status: TaskStatus,
        action: ConflictAction,
    },
    #[error("clock went backwards")]
    Clock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskOrigin {
    Heartbeat,
    Chat,
    Tool,
    Mesh,
    Api,
    Eval,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskRecord {
    pub id: Uuid,
    pub goal: String,
    pub context_refs: Vec<ContextRef>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub spec: TaskSpec,
    pub executor: TaskExecutorKind,
    pub executor_target: Option<String>,
    pub requested_by: Option<String>,
    pub agent_id: String,
    pub origin: TaskOrigin,
    pub parent_task_id: Option<Uuid>,
    pub chain_depth: u32,
    pub budget: TaskBudget,
    pub node_affinity: Option<String>,
    pub usage: Option<TaskUsage>,
    pub status: TaskStatus,
    pub attempt: u32,
    pub max_attempts: u32,
    pub result: Option<TaskResult>,
    pub error: Option<String>,
    pub conversation_id: Option<Uuid>,
    pub session_key: Option<String>,
    pub pending_message: Option<String>,
    pub claimed_by: Option<String>,
    pub harness_session_ids: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<i32>,
    pub eval: Option<VerifierResult>,
    pub eval_attempt: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finish {
    pub status: TaskStatus,
    pub result: Option<TaskResult>,
    pub error: Option<String>,
    pub usage: Option<TaskUsage>,
    pub completed_at: DateTime<Utc>,
    pub duration_ms: Option<i32>,
}

#[must_use]
pub fn terminal(status: TaskStatus) -> bool {
    match status {
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Killed | TaskStatus::Timeout => {
            true
        }
        TaskStatus::Queued | TaskStatus::Running | TaskStatus::AwaitingInput => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskJobPayload {
    pub task_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    pub name: JobName,
    pub payload: TaskJobPayload,
    pub key: String,
    pub key_mode: JobKeyMode,
    pub priority: i16,
    pub max_attempts: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimSelector {
    pub names: Vec<String>,
    pub only_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimedJob {
    pub id: i64,
    pub identifier: String,
    pub payload_text: String,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredJob {
    pub id: i64,
    pub identifier: String,
    pub payload: TaskJobPayload,
    pub key: Option<String>,
    pub priority: i16,
    pub max_attempts: i32,
    pub attempts: u32,
    pub last_error: Option<String>,
}

#[async_trait]
pub trait TaskStore: std::fmt::Debug + Send + Sync {
    async fn insert(&self, task: &TaskRecord) -> Result<(), TaskError>;

    async fn load(&self, id: Uuid) -> Result<TaskRecord, TaskError>;

    async fn mark_running(
        &self,
        id: Uuid,
        claimed_by: &str,
        node: &str,
        now: DateTime<Utc>,
    ) -> Result<TaskRecord, TaskError>;

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, TaskError>;

    async fn sweep(&self, worker: &str, now: DateTime<Utc>) -> Result<(), TaskError>;

    async fn kill_queued_by_session(
        &self,
        session_key: &str,
        now: DateTime<Utc>,
    ) -> Result<(), TaskError>;

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, TaskError>;

    async fn set_pending(&self, id: Uuid, message: &str) -> Result<(), TaskError>;

    async fn add_harness_session(&self, id: Uuid, session: &str) -> Result<(), TaskError>;
}

#[async_trait]
pub trait TaskJobs: std::fmt::Debug + Send + Sync {
    async fn enqueue(&self, job: &NewJob) -> Result<i64, TaskError>;

    async fn claim(&self, selector: &ClaimSelector) -> Result<Option<ClaimedJob>, TaskError>;

    async fn complete(&self, id: i64) -> Result<(), TaskError>;

    async fn fail(&self, id: i64, error: &str) -> Result<(), TaskError>;

    async fn read(&self, id: i64) -> Result<Option<StoredJob>, TaskError>;

    async fn await_notification(&self, cancel: CancellationToken) -> Result<bool, TaskError>;
}
