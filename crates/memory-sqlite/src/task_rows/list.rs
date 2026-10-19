use kernel::tasks::TaskError;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskRecord;
use rusqlite::Connection;
use rusqlite::params;

use super::decode::{map_row, record_from};
use super::{ensure, sql_error};

const LIST: &str =
    "SELECT id, goal, context_refs, acceptance_criteria, spec, executor, executor_target,
    requested_by, agent_id, origin, parent_task_id, chain_depth, budget, node_affinity, usage,
    status, attempt, max_attempts, result, error, conversation_id, session_key, pending_message,
    claimed_by, harness_session_ids, created_at, started_at, last_heartbeat_at, completed_at,
    duration_ms, eval, eval_attempt
    FROM rh_tasks WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR agent_id = ?2)
    ORDER BY created_at DESC, id DESC LIMIT COALESCE(?3, -1)";

pub(crate) fn list_tasks(
    conn: &Connection,
    filter: &TaskListFilter,
) -> Result<Vec<TaskRecord>, TaskError> {
    ensure(conn, "list tasks")?;
    let status = filter.status.map(|status| status.to_string());
    let limit = filter.limit.map(i64::from);
    let mut statement = conn
        .prepare(LIST)
        .map_err(|source| sql_error("list tasks", source))?;
    let rows = statement
        .query_map(params![status, filter.agent_id, limit], map_row)
        .map_err(|source| sql_error("list tasks", source))?;
    let mut tasks = Vec::new();
    for row in rows {
        let raw = row.map_err(|source| sql_error("list tasks", source))?;
        tasks.push(record_from(raw)?);
    }
    Ok(tasks)
}
