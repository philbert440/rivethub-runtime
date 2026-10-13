use crate::JsonObject;
use crate::events::AgentEvent;
use crate::session_id::SessionId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use strum::{Display, EnumIter, EnumString};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskExecutorKind {
    ChatLoop,
    HarnessSession,
    Mesh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    Queued,
    Running,
    AwaitingInput,
    Completed,
    Failed,
    Killed,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TaskVerdict {
    Completed,
    Failed,
    Killed,
    Timeout,
    BudgetExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextRefKind {
    Conversation,
    Message,
    Task,
    File,
    Url,
    Wiki,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRef {
    pub kind: ContextRefKind,
    pub r#ref: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CriterionKind {
    Manual,
    Automated,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceCriterion {
    pub id: String,
    pub description: String,
    pub kind: CriterionKind,
    pub check: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBudget {
    pub max_usd: Option<f64>,
    pub max_tokens: Option<u64>,
    pub max_turns: Option<u32>,
    pub max_wall_clock_ms: Option<u64>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
    pub turns: u32,
    pub wall_clock_ms: u64,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Display, EnumString, EnumIter,
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum Effort {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskIsolation {
    Inherit,
    Isolated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskPromptMode {
    Task,
    Heartbeat,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSpec {
    pub task_id: String,
    pub agent_id: String,
    pub goal: String,
    pub resolved_context: Vec<ContextRef>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub budget: TaskBudget,
    pub tools: Option<Vec<String>>,
    pub working_dir: Option<String>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub system_prompt_append: Option<String>,
    pub isolation: Option<TaskIsolation>,
    pub prompt_mode: Option<TaskPromptMode>,
    pub exclude_tools: Option<Vec<String>>,
    pub resume_message: Option<String>,
    pub session: String,
    pub permission_decisions: Option<Vec<TaskPermissionDecision>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskPermissionOutcome {
    Allow,
    Deny,
    Timeout,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPermissionDecision {
    pub request_id: String,
    pub tool: String,
    pub decision: TaskPermissionOutcome,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub at: DateTime<Utc>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    File,
    Url,
    Commit,
    Message,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifact {
    pub kind: ArtifactKind,
    pub r#ref: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CriterionReport {
    pub id: String,
    pub met: bool,
    pub evidence: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub verdict: TaskVerdict,
    pub summary: String,
    pub output: Option<String>,
    pub artifacts: Vec<TaskArtifact>,
    pub criteria_self_report: Option<Vec<CriterionReport>>,
    pub usage: TaskUsage,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifierVerdict {
    Verified,
    Refuted,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifierResult {
    pub verdict: VerifierVerdict,
    pub summary: String,
    pub criteria_report: Vec<CriterionReport>,
    pub refutation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvalVerdict {
    Verified,
    Refuted,
    Escalated,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalOutcome {
    pub verdict: EvalVerdict,
    pub attempts: u32,
    pub verifier_task_ids: Vec<String>,
    pub criteria_report: Vec<CriterionReport>,
    pub diverged: bool,
    pub escalated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpInjection {
    Flag,
    CwdFile,
    PersistentConfig,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessExecutorCapabilities {
    #[serde(flatten)]
    pub turn: ExecutorTurnCapabilities,
    #[serde(flatten)]
    pub result: ExecutorResultCapabilities,
    #[serde(flatten)]
    pub interaction: ExecutorInteractionCapabilities,
    pub mcp_injection: McpInjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorTurnCapabilities {
    pub steerable: bool,
    pub multi_turn: bool,
    pub structured_stream: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorResultCapabilities {
    pub usage_in_result: bool,
    pub session_id_capture: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorInteractionCapabilities {
    pub slash_commands: bool,
    pub effort_selection: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskApprovalRequest {
    pub task_id: String,
    pub request_id: String,
    pub name: String,
    pub input: JsonObject,
    pub tool_call_id: Option<String>,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TaskEvent {
    #[serde(rename = "den")]
    Session {
        #[serde_as(as = "serde_with::TimestampMilliSeconds")]
        ts: DateTime<Utc>,
        event: AgentEvent,
    },
    #[serde(rename = "turn.start")]
    TurnStart {
        #[serde_as(as = "serde_with::TimestampMilliSeconds")]
        ts: DateTime<Utc>,
        turn: u32,
    },
    #[serde(rename = "turn.end", rename_all = "camelCase")]
    TurnEnd {
        #[serde_as(as = "serde_with::TimestampMilliSeconds")]
        ts: DateTime<Utc>,
        turn: u32,
        usage: TaskUsage,
        harness_session_id: Option<SessionId>,
    },
    #[serde(rename = "session.spawned", rename_all = "camelCase")]
    SessionSpawned {
        #[serde_as(as = "serde_with::TimestampMilliSeconds")]
        ts: DateTime<Utc>,
        native_session_id: String,
        model: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Cost {
        #[serde_as(as = "serde_with::TimestampMilliSeconds")]
        ts: DateTime<Utc>,
        delta_usd: f64,
        total_usd: f64,
    },
    Log {
        #[serde_as(as = "serde_with::TimestampMilliSeconds")]
        ts: DateTime<Utc>,
        level: LogLevel,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalRequest {
        #[serde_as(as = "serde_with::TimestampMilliSeconds")]
        ts: DateTime<Utc>,
        task_id: String,
        request_id: String,
        name: String,
        input: JsonObject,
        tool_call_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalResolved {
        #[serde_as(as = "serde_with::TimestampMilliSeconds")]
        ts: DateTime<Utc>,
        task_id: String,
        request_id: String,
        decision: TaskPermissionOutcome,
    },
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWire {
    pub id: String,
    pub goal: String,
    pub context_refs: Vec<ContextRef>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub spec: TaskSpec,
    pub executor: TaskExecutorKind,
    pub executor_target: Option<String>,
    pub agent_id: String,
    pub requested_by: Option<String>,
    pub origin: String,
    pub parent_task_id: Option<String>,
    pub chain_depth: u32,
    pub node_affinity: Option<String>,
    pub claimed_by: Option<String>,
    pub budget: TaskBudget,
    pub usage: Option<TaskUsage>,
    pub status: TaskStatus,
    pub attempt: u32,
    pub max_attempts: u32,
    pub pending_message: Option<String>,
    pub error: Option<String>,
    pub result: Option<TaskResult>,
    pub conversation_id: Option<String>,
    pub session_key: Option<String>,
    pub harness_session_ids: Vec<String>,
    pub eval: Option<VerifierResult>,
    pub eval_attempt: u32,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    #[serde_as(as = "Option<serde_with::TimestampMilliSeconds>")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[serde_as(as = "Option<serde_with::TimestampMilliSeconds>")]
    pub last_heartbeat_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[serde_as(as = "Option<serde_with::TimestampMilliSeconds>")]
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub pending_approvals: Option<Vec<TaskApprovalRequest>>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCreateRequest {
    pub goal: String,
    pub agent_id: String,
    pub executor: Option<TaskExecutorKind>,
    pub executor_target: Option<String>,
    pub requested_by: Option<String>,
    pub parent_task_id: Option<String>,
    pub chain_depth: Option<u32>,
    pub node_affinity: Option<String>,
    pub spec: Option<TaskSpec>,
    pub budget: Option<TaskBudget>,
    pub context_refs: Option<Vec<ContextRef>>,
    pub acceptance_criteria: Option<Vec<AcceptanceCriterion>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResponse {
    pub task: TaskWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksListResponse {
    pub tasks: Vec<TaskWire>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSteerAccepted {
    pub ok: bool,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskKillResponse {
    pub ok: bool,
    pub prior: Option<TaskStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskApprovalAccepted {
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskApprovalWaitResponse {
    pub approval: TaskApprovalRequest,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWaitTimeoutResponse {
    pub error: String,
    pub task: Option<TaskWire>,
}
