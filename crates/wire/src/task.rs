use crate::JsonObject;
use crate::events::AgentEvent;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextRef {
    pub kind: ContextRefKind,
    pub r#ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CriterionKind {
    Manual,
    Automated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceCriterion {
    pub id: String,
    pub description: String,
    pub kind: CriterionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskBudget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_clock_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub turns: u32,
    pub wall_clock_ms: i64,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSpec {
    pub task_id: String,
    pub agent_id: String,
    pub goal: String,
    pub resolved_context: Vec<ContextRef>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub budget: TaskBudget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt_append: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<TaskIsolation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_mode: Option<TaskPromptMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclude_tools: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_message: Option<String>,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_decisions: Option<Vec<TaskPermissionDecision>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskPermissionOutcome {
    Allow,
    Deny,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPermissionDecision {
    pub request_id: String,
    pub tool: String,
    pub decision: TaskPermissionOutcome,
    pub at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskArtifact {
    pub kind: ArtifactKind,
    pub r#ref: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CriterionReport {
    pub id: String,
    pub met: bool,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub verdict: TaskVerdict,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    pub artifacts: Vec<TaskArtifact>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criteria_self_report: Option<Vec<CriterionReport>>,
    pub usage: TaskUsage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifierVerdict {
    Verified,
    Refuted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifierResult {
    pub verdict: VerifierVerdict,
    pub summary: String,
    pub criteria_report: Vec<CriterionReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refutation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EvalVerdict {
    Verified,
    Refuted,
    Escalated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalOutcome {
    pub verdict: EvalVerdict,
    pub attempts: u32,
    pub verifier_task_ids: Vec<String>,
    pub criteria_report: Vec<CriterionReport>,
    pub diverged: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskApprovalRequest {
    pub task_id: String,
    pub request_id: String,
    pub name: String,
    pub input: JsonObject,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TaskEvent {
    #[serde(rename = "den")]
    Session { ts: i64, event: AgentEvent },
    #[serde(rename = "turn.start")]
    TurnStart { ts: i64, turn: u32 },
    #[serde(rename = "turn.end", rename_all = "camelCase")]
    TurnEnd {
        ts: i64,
        turn: u32,
        usage: TaskUsage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        harness_session_id: Option<String>,
    },
    #[serde(rename = "session.spawned", rename_all = "camelCase")]
    SessionSpawned {
        ts: i64,
        native_session_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Cost {
        ts: i64,
        delta_usd: f64,
        total_usd: f64,
    },
    Log {
        ts: i64,
        level: LogLevel,
        message: String,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalRequest {
        ts: i64,
        task_id: String,
        request_id: String,
        name: String,
        input: JsonObject,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ApprovalResolved {
        ts: i64,
        task_id: String,
        request_id: String,
        decision: TaskPermissionOutcome,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWire {
    pub id: String,
    pub goal: String,
    pub context_refs: Vec<ContextRef>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub spec: TaskSpec,
    pub executor: TaskExecutorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_target: Option<String>,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<String>,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    pub chain_depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_affinity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    pub budget: TaskBudget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TaskUsage>,
    pub status: TaskStatus,
    pub attempt: u32,
    pub max_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<TaskResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    pub harness_session_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eval: Option<VerifierResult>,
    pub eval_attempt: u32,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_heartbeat_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_approvals: Option<Vec<TaskApprovalRequest>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskCreateRequest {
    pub goal: String,
    pub agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<TaskExecutorKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain_depth: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_affinity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<TaskSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<TaskBudget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_refs: Option<Vec<ContextRef>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWaitTimeoutResponse {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskWire>,
}
