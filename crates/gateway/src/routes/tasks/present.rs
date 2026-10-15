use kernel::tasks::TaskRecord;
use uuid::Uuid;
use wire::task::TaskApprovalRequest;
use wire::task::TaskWire;

use super::error::TaskHttpError;

pub(super) fn to_wire(
    task: &TaskRecord,
    pending: &[TaskApprovalRequest],
) -> Result<TaskWire, TaskHttpError> {
    let pending_approvals = if pending.is_empty() {
        None
    } else {
        Some(pending.to_vec())
    };
    Ok(TaskWire {
        id: task.id.to_string(),
        goal: task.goal.clone(),
        context_refs: task.context_refs.clone(),
        acceptance_criteria: task.acceptance_criteria.clone(),
        spec: task.spec.clone(),
        executor: task.executor,
        executor_target: task.executor_target.clone(),
        agent_id: task.agent_id.clone(),
        requested_by: task.requested_by.clone(),
        origin: task.origin.to_string(),
        parent_task_id: text_id(task.parent_task_id),
        chain_depth: task.chain_depth,
        node_affinity: task.node_affinity.clone(),
        claimed_by: task.claimed_by.clone(),
        budget: task.budget,
        usage: task.usage,
        status: task.status,
        attempt: task.attempt,
        max_attempts: task.max_attempts,
        pending_message: task.pending_message.clone(),
        error: task.error.clone(),
        result: task.result.clone(),
        conversation_id: text_id(task.conversation_id),
        session_key: task.session_key.clone(),
        harness_session_ids: task.harness_session_ids.clone(),
        eval: task.eval.clone(),
        eval_attempt: task.eval_attempt,
        created_at: task.created_at,
        started_at: task.started_at,
        last_heartbeat_at: task.last_heartbeat_at,
        completed_at: task.completed_at,
        duration_ms: widen_duration(task.duration_ms)?,
        pending_approvals,
    })
}

fn text_id(id: Option<Uuid>) -> Option<String> {
    id.map(|value| value.to_string())
}

fn widen_duration(value: Option<i32>) -> Result<Option<u64>, TaskHttpError> {
    match value {
        None => Ok(None),
        Some(ms) => {
            let widened = u64::try_from(ms)
                .map_err(|_| TaskHttpError::Failed("stored duration is negative".to_owned()))?;
            Ok(Some(widened))
        }
    }
}
