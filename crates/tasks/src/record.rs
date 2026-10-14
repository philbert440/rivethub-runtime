use chrono::DateTime;
use chrono::Utc;
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

use crate::origin::TaskOrigin;

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

pub(crate) fn terminal(status: TaskStatus) -> bool {
    match status {
        TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Killed | TaskStatus::Timeout => {
            true
        }
        TaskStatus::Queued | TaskStatus::Running | TaskStatus::AwaitingInput => false,
    }
}
