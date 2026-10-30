use kernel::tasks::Task;
use kernel::tasks::TaskStatus;
use kernel::tasks::Verdict;
use wire::task::AcceptanceCriterion;
use wire::task::CriterionKind;
use wire::task::TaskBudget;
use wire::task::TaskResult;
use wire::task::TaskStatus as WireStatus;
use wire::task::TaskUsage;
use wire::task::TaskVerdict;
use wire::task::TaskWire;

use super::error::TaskHttpError;

pub(super) fn to_wire(task: &Task) -> Result<TaskWire, TaskHttpError> {
    let usage = task.usage.map(|usage| TaskUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage
            .input_tokens
            .zip(usage.output_tokens)
            .and_then(|(input, output)| input.checked_add(output)),
        turns: usage.turns,
        wall_clock_ms: usage.wall_clock_ms,
    });
    let result = match (&task.result, usage) {
        (Some(outcome), Some(usage)) => Some(TaskResult {
            verdict: verdict_out(outcome.verdict),
            summary: outcome.summary.clone(),
            usage,
            error: task.error.clone(),
        }),
        (Some(_) | None, _) => None,
    };
    Ok(TaskWire {
        id: task.id.to_string(),
        goal: task.goal.clone(),
        acceptance_criteria: task
            .criteria
            .iter()
            .map(|criterion| AcceptanceCriterion {
                id: criterion.id.clone(),
                description: criterion.description.clone(),
                kind: CriterionKind::Manual,
                check: None,
            })
            .collect(),
        agent_id: task.target.agent.clone(),
        node: task.target.node.clone(),
        harness: task.target.harness.map(crate::tasks::harness_out),
        model: task.target.model.clone(),
        effort: task.target.effort.clone(),
        requested_by: task.requested_by.clone(),
        parent_session_id: task.parent.session.clone(),
        parent_task_id: task.parent.task.map(|id| id.to_string()),
        depth: task.parent.depth,
        child_session_id: task.child.clone(),
        budget: TaskBudget {
            max_turns: task.budget.max_turns,
            max_wall_clock_ms: task
                .budget
                .wall_clock
                .map(|wall| u64::try_from(wall.as_millis()))
                .transpose()
                .map_err(|_| TaskHttpError::Failed("stored budget".to_owned()))?,
        },
        usage,
        status: status_out(task.status),
        error: task.error.clone(),
        result,
        created_at: task.created_at,
        started_at: task.started_at,
        completed_at: task.completed_at,
        duration_ms: task
            .started_at
            .zip(task.completed_at)
            .map(|(started, ended)| {
                ended
                    .signed_duration_since(started)
                    .num_milliseconds()
                    .max(0)
                    .unsigned_abs()
            }),
    })
}

pub(crate) const fn status_out(status: TaskStatus) -> WireStatus {
    match status {
        TaskStatus::Queued => WireStatus::Queued,
        TaskStatus::Running => WireStatus::Running,
        TaskStatus::AwaitingInput => WireStatus::AwaitingInput,
        TaskStatus::Completed => WireStatus::Completed,
        TaskStatus::Failed => WireStatus::Failed,
        TaskStatus::Killed => WireStatus::Killed,
        TaskStatus::Timeout => WireStatus::Timeout,
    }
}

const fn verdict_out(verdict: Verdict) -> TaskVerdict {
    match verdict {
        Verdict::Completed => TaskVerdict::Completed,
        Verdict::Failed => TaskVerdict::Failed,
        Verdict::Killed => TaskVerdict::Killed,
        Verdict::Timeout => TaskVerdict::Timeout,
        Verdict::BudgetExceeded => TaskVerdict::BudgetExceeded,
    }
}
