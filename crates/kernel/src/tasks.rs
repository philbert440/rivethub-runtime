use std::fmt::Debug;
use std::time::Duration;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use strum::Display;
use strum::EnumString;
use thiserror::Error;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::gateway::catalog::CatalogAgent;
use wire::gateway::catalog::CatalogCommand;
use wire::gateway::catalog::CatalogExecutorEntry;
use wire::gateway::catalog::CatalogSkillEntry;
use wire::harness::HarnessId;
use wire::task::AcceptanceCriterion;
use wire::task::ContextRef;
use wire::task::TaskApprovalRequest;
use wire::task::TaskBudget;
use wire::task::TaskExecutorKind;
use wire::task::TaskPermissionDecision;
use wire::task::TaskResult;
use wire::task::TaskSpec;
use wire::task::TaskStatus;
use wire::task::TaskUsage;
use wire::task::VerifierResult;

use crate::BoxError;
use crate::queue::JobQueueError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictAction {
    Run,
    Steer,
    Kill,
    Release,
}

impl std::fmt::Display for ConflictAction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Run => "run",
            Self::Steer => "steer",
            Self::Kill => "kill",
            Self::Release => "release",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogList {
    Skills,
    Commands,
}

impl std::fmt::Display for CatalogList {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Skills => "skills",
            Self::Commands => "harness commands",
        })
    }
}

#[derive(Debug, Error)]
pub enum TaskError {
    #[error("database connection failed")]
    Unavailable(#[source] BoxError),
    #[error("{0} are unavailable")]
    UnavailableCatalog(CatalogList),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
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

    async fn release_running(&self, id: Uuid) -> Result<(), TaskError>;

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

    async fn append_permission(
        &self,
        id: Uuid,
        decision: &TaskPermissionDecision,
    ) -> Result<(), TaskError>;
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentDirectory {
    pub presets: Vec<PresetAgent>,
    pub runtime: Vec<RuntimeAgent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresetAgent {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    pub local: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeAgent {
    pub id: String,
    pub nodes: Vec<AgentNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentNode {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DelegateRequest {
    pub requested_by: Option<String>,
    pub target: String,
    pub prompt: String,
    pub model: Option<String>,
    pub node: Option<String>,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelegateOutcome {
    pub id: Uuid,
    pub status: TaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskListFilter {
    pub status: Option<TaskStatus>,
    pub agent_id: Option<String>,
    pub limit: Option<u32>,
}

#[async_trait]
pub trait TaskQuery: Debug + Send + Sync {
    async fn list(&self, filter: &TaskListFilter) -> Result<Vec<TaskRecord>, TaskError>;
}

pub trait TaskWatch: Debug + Send + Sync {
    fn subscribe(&self) -> watch::Receiver<u64>;
}

#[async_trait]
pub trait CatalogSource: Debug + Send + Sync {
    async fn agents(&self) -> Result<Vec<CatalogAgent>, TaskError>;

    async fn executors(&self) -> Result<Vec<CatalogExecutorEntry>, TaskError>;

    async fn tools(&self) -> Result<Vec<String>, TaskError>;

    async fn skills(&self) -> Result<Vec<CatalogSkillEntry>, TaskError>;

    async fn commands(&self, executor: &str) -> Result<Vec<CatalogCommand>, TaskError>;
}

#[async_trait]
pub trait TaskAgents: Debug + Send + Sync {
    async fn contains(&self, agent_id: &str) -> Result<bool, TaskError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    Allow,
    Deny,
}

#[derive(Debug, Error)]
pub enum ApprovalError {
    #[error("unknown approval")]
    Unknown,
}

#[async_trait]
pub trait ApprovalBroker: Debug + Send + Sync {
    async fn pending(&self, task: Uuid) -> Result<Vec<TaskApprovalRequest>, ApprovalError>;

    async fn resolve(
        &self,
        task: Uuid,
        request_id: &str,
        decision: ApprovalDecision,
    ) -> Result<(), ApprovalError>;

    async fn deny_pending(&self, task: Uuid, reason: &str) -> Result<(), ApprovalError>;
}
