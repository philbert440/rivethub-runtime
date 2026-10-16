use serde::Deserialize;
use serde::Serialize;

use crate::Extra;
use crate::task::AcceptanceCriterion;
use crate::task::ContextRef;
use crate::task::Effort;
use crate::task::TaskBudget;
use crate::task::TaskIsolation;
use crate::task::TaskPermissionDecision;
use crate::task::TaskPromptMode;
use crate::task::TaskSpec;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct TaskOverrides {
    pub preset_id: Option<String>,
    pub preset_name: Option<String>,
    pub permission_decisions: Option<Vec<TaskPermissionDecision>>,
    pub audit_only: Option<bool>,
    pub working_dir: Option<String>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub system_prompt_append: Option<String>,
    pub isolation: Option<TaskIsolation>,
    pub prompt_mode: Option<TaskPromptMode>,
    pub exclude_tools: Option<Vec<String>>,
    pub resume_message: Option<String>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug)]
pub struct TaskColumns {
    pub task_id: String,
    pub agent_id: String,
    pub goal: String,
    pub resolved_context: Vec<ContextRef>,
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
    pub budget: TaskBudget,
    pub session: String,
}

impl TaskOverrides {
    pub fn from_spec(spec: &TaskSpec) -> Self {
        Self {
            preset_id: spec.preset_id.clone(),
            preset_name: spec.preset_name.clone(),
            permission_decisions: spec.permission_decisions.clone(),
            audit_only: spec.audit_only,
            working_dir: spec.working_dir.clone(),
            model: spec.model.clone(),
            effort: spec.effort,
            system_prompt_append: spec.system_prompt_append.clone(),
            isolation: spec.isolation,
            prompt_mode: spec.prompt_mode,
            exclude_tools: spec.exclude_tools.clone(),
            resume_message: spec.resume_message.clone(),
            extra: spec.extra.clone(),
        }
    }

    pub fn into_spec(self, columns: TaskColumns) -> TaskSpec {
        TaskSpec {
            task_id: columns.task_id,
            agent_id: columns.agent_id,
            goal: columns.goal,
            resolved_context: columns.resolved_context,
            acceptance_criteria: columns.acceptance_criteria,
            budget: columns.budget,
            tools: None,
            working_dir: self.working_dir,
            model: self.model,
            effort: self.effort,
            system_prompt_append: self.system_prompt_append,
            isolation: self.isolation,
            prompt_mode: self.prompt_mode,
            exclude_tools: self.exclude_tools,
            resume_message: self.resume_message,
            session: columns.session,
            permission_decisions: self.permission_decisions,
            preset_id: self.preset_id,
            preset_name: self.preset_name,
            audit_only: self.audit_only,
            extra: self.extra,
        }
    }
}
