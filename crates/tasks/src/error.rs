use kernel::tasks::ConflictAction;
use kernel::tasks::TaskError;
use thiserror::Error;
use uuid::Uuid;
use wire::gateway::GatewayError;
use wire::task::TaskStatus;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database connection failed")]
    Connection(#[source] kernel::BoxError),
    #[error("database operation failed: {operation}")]
    Operation {
        operation: &'static str,
        #[source]
        source: kernel::BoxError,
    },
    #[error("graphile worker queue is absent")]
    QueueAbsent,
    #[error("enqueue failed for task {id}")]
    Enqueue {
        id: Uuid,
        #[source]
        source: Box<Self>,
    },
    #[error("{context} failed ({primary}); cleanup also failed ({cleanup})")]
    Cleanup {
        context: &'static str,
        primary: String,
        cleanup: String,
    },
    #[error("job payload is not a task job")]
    Payload(#[source] Box<serde_json::Error>),
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
    #[error("executor is required")]
    ExecutorRequired,
    #[error("goal is required")]
    GoalRequired,
    #[error("agent id is required")]
    AgentRequired,
    #[error("spec session is empty")]
    EmptySession,
    #[error("chain depth is required when a parent task is set")]
    ChainDepthRequired,
    #[error("parent task id is not a uuid")]
    ParentId,
    #[error("parent task is missing")]
    MissingParent,
    #[error("chain depth {got} does not follow parent depth {parent}")]
    ChainDepth { got: u32, parent: u32 },
    #[error("agent {agent} is not a local agent or a preset")]
    UnknownTarget { agent: String },
    #[error("agent {agent} matches both a local agent and a preset")]
    AmbiguousTarget { agent: String },
    #[error("mesh node {node} is not online")]
    MeshUnavailable { node: String },
    #[error("agent {agent} is not on mesh node {node}")]
    MeshAgent { node: String, agent: String },
    #[error("mesh secret is not configured")]
    MeshUnauthorized,
    #[error("mesh address is invalid")]
    MeshAddress,
    #[error("harness is required to open a harness session")]
    HarnessRequired,
    #[error("path escapes the files root")]
    PathEscape,
    #[error("working directory does not exist")]
    PathMissing,
    #[error("path lookup failed")]
    PathIo(#[source] std::io::Error),
    #[error("blocking path lookup failed")]
    Blocking(#[source] tokio::task::JoinError),
    #[error("a files root is required to contain the working directory")]
    FilesRootRequired,
    #[error("unauthorized")]
    Unauthorized,
    #[error("gateway returned {status}")]
    Rejected {
        status: u16,
        error: Option<Box<GatewayError>>,
    },
    #[error("gateway response was not the expected document")]
    Response,
    #[error("http request failed")]
    Http(#[source] Box<reqwest::Error>),
    #[error("gateway timed out")]
    Timeout,
    #[error("redirect refused")]
    Redirect(#[source] Box<reqwest::Error>),
    #[error("session path segment is invalid")]
    SessionSegment,
    #[error("budget exceeded")]
    Budget,
    #[error("token pricing is missing or invalid")]
    Pricing,
    #[error("job name is invalid")]
    JobName(#[source] kernel::queue::JobNameError),
    #[error("task run was cancelled")]
    Cancelled,
    #[error("heartbeat agent is empty")]
    HeartbeatAgent,
    #[error("gateway base url is invalid")]
    BaseUrl,
    #[error("clock went backwards")]
    Clock,
    #[error("node name is empty")]
    NodeRequired,
}

impl From<TaskError> for Error {
    fn from(value: TaskError) -> Self {
        match value {
            TaskError::Unavailable(source) => Self::Connection(source),
            TaskError::Operation { operation, source } => Self::Operation { operation, source },
            TaskError::QueueAbsent => Self::QueueAbsent,
            TaskError::Cleanup {
                context,
                primary,
                cleanup,
            } => Self::Cleanup {
                context,
                primary,
                cleanup,
            },
            TaskError::Payload(source) => match source.downcast::<serde_json::Error>() {
                Ok(error) => Self::Payload(error),
                Err(other) => Self::Stored(other.to_string()),
            },
            TaskError::Stored(message) => Self::Stored(message),
            TaskError::Missing { id } => Self::Missing { id },
            TaskError::MissingJob { id } => Self::MissingJob { id },
            TaskError::Conflict { id, status, action } => Self::Conflict { id, status, action },
            TaskError::Clock => Self::Clock,
        }
    }
}
