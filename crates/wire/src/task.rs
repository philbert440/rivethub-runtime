use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use strum::{Display, EnumIter, EnumString};

use crate::harness::HarnessId;
use crate::stream::ThinkingLevel;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    Display,
    EnumString,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum TaskStatus {
    Queued,
    Running,
    AwaitingInput,
    Completed,
    Failed,
    Killed,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TaskVerdict {
    Completed,
    Failed,
    Killed,
    Timeout,
    BudgetExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CriterionKind {
    Manual,
    Automated,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceCriterion {
    pub id: String,
    pub description: String,
    pub kind: CriterionKind,
    #[serde(default)]
    pub check: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskBudget {
    pub max_turns: Option<u32>,
    pub max_wall_clock_ms: Option<u64>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskUsage {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    pub turns: u32,
    pub wall_clock_ms: u64,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    Display,
    EnumString,
    EnumIter,
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum Effort {
    Low,
    Medium,
    High,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskResult {
    pub verdict: TaskVerdict,
    pub summary: String,
    pub usage: TaskUsage,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    strum::Display,
    strum::EnumString,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskWire {
    pub id: String,
    pub goal: String,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub agent_id: String,
    pub node: String,
    #[serde(default)]
    pub harness: Option<HarnessId>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub requested_by: Option<String>,
    #[serde(default)]
    pub parent_session_id: Option<String>,
    #[serde(default)]
    pub parent_task_id: Option<String>,
    pub depth: u8,
    #[serde(default)]
    pub child_session_id: Option<String>,
    pub budget: TaskBudget,
    #[serde(default)]
    pub usage: Option<TaskUsage>,
    pub status: TaskStatus,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub result: Option<TaskResult>,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    #[serde_as(as = "Option<serde_with::TimestampMilliSeconds>")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    #[serde_as(as = "Option<serde_with::TimestampMilliSeconds>")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskCreateRequest {
    pub goal: Option<String>,
    pub agent_id: Option<String>,
    pub node_affinity: Option<String>,
    pub acceptance_criteria: Option<Vec<AcceptanceCriterion>>,
    pub budget: Option<TaskBudget>,
    pub parent_session_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<ThinkingLevel>,
    pub working_dir: Option<String>,
    pub requested_by: Option<String>,
    #[serde(flatten)]
    pub extra: crate::Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskResponse {
    pub task: TaskWire,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TasksListResponse {
    pub tasks: Vec<TaskWire>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskKillResponse {
    pub ok: bool,
    pub prior: Option<TaskStatus>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskWaitTimeoutResponse {
    pub error: String,
    #[serde(default)]
    pub task: Option<TaskWire>,
}
