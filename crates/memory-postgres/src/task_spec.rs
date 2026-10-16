use uuid::Uuid;
use wire::task::AcceptanceCriterion;
use wire::task::ContextRef;
use wire::task::TaskBudget;
use wire::task::TaskSpec;
use wire::task_overrides::TaskColumns;
use wire::task_overrides::TaskOverrides;

use kernel::tasks::TaskError;

#[derive(Debug)]
pub(crate) struct LoadedColumns<'a> {
    pub id: Uuid,
    pub agent_id: &'a str,
    pub goal: &'a str,
    pub resolved_context: &'a [ContextRef],
    pub acceptance_criteria: &'a [AcceptanceCriterion],
    pub budget: TaskBudget,
    pub session_key: Option<&'a str>,
}

pub(crate) fn bag_json(spec: &TaskSpec) -> Result<String, TaskError> {
    if spec.tools.is_some() {
        return Err(TaskError::Stored(
            "tools is not a column override".to_string(),
        ));
    }
    serde_json::to_string(&TaskOverrides::from_spec(spec))
        .map_err(|source| TaskError::Stored(source.to_string()))
}

pub(crate) fn assemble_spec(
    columns: &LoadedColumns<'_>,
    overrides: TaskOverrides,
) -> Result<TaskSpec, TaskError> {
    let Some(session) = columns.session_key else {
        return Err(TaskError::Stored("session key is absent".to_string()));
    };
    Ok(overrides.into_spec(TaskColumns {
        task_id: columns.id.to_string(),
        agent_id: columns.agent_id.to_owned(),
        goal: columns.goal.to_owned(),
        resolved_context: columns.resolved_context.to_vec(),
        acceptance_criteria: columns.acceptance_criteria.to_vec(),
        budget: columns.budget,
        session: session.to_owned(),
    }))
}
