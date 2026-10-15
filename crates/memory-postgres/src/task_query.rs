use kernel::tasks::TaskError;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskRecord;
use sqlx::PgPool;

use crate::task_rows::decode_task;

const LIST_TASKS: &str = concat!(
    "SELECT id, goal, context_refs, acceptance_criteria, spec, executor, executor_target, ",
    "requested_by, agent_id, origin, parent_task_id, chain_depth, budget, node_affinity, usage, ",
    "status, attempt, max_attempts, result, error, conversation_id, session_key, pending_message, ",
    "claimed_by, harness_session_ids, created_at, started_at, last_heartbeat_at, completed_at, ",
    "duration_ms, eval, eval_attempt FROM rh_tasks ",
    "WHERE ($1::text IS NULL OR status = $1) AND ($2::text IS NULL OR agent_id = $2) ",
    "ORDER BY created_at DESC, id DESC LIMIT $3"
);

pub(crate) async fn list_tasks(
    pool: &PgPool,
    filter: &TaskListFilter,
) -> Result<Vec<TaskRecord>, TaskError> {
    let status = filter.status.map(|status| status.to_string());
    let limit = filter.limit.map(i64::from);
    let rows = sqlx::query(LIST_TASKS)
        .bind(status)
        .bind(filter.agent_id.as_deref())
        .bind(limit)
        .fetch_all(pool)
        .await
        .map_err(|source| TaskError::Operation {
            operation: "list tasks",
            source: Box::new(source),
        })?;
    rows.iter()
        .map(|row| decode_task(row, "list tasks"))
        .collect()
}
