use chrono::DateTime;
use chrono::Utc;
use serde::Serialize;
use sqlx::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;
use wire::task::AcceptanceCriterion;
use wire::task::ContextRef;
use wire::task::EvalOutcome;
use wire::task::TaskBudget;
use wire::task::TaskPermissionDecision;
use wire::task::TaskResult;
use wire::task::TaskStatus;
use wire::task::TaskUsage;
use wire::task_overrides::TaskOverrides;

use crate::task_jobs::cell;
use crate::task_jobs::map_sql;
use kernel::tasks::ConflictAction;
use kernel::tasks::Finish;
use kernel::tasks::TaskError;
use kernel::tasks::TaskRecord;

const LOAD_TASK: &str = concat!(
    "SELECT id, goal, context_refs, acceptance_criteria, spec, executor, executor_target, ",
    "requested_by, agent_id, origin, parent_task_id, chain_depth, budget, node_affinity, usage, ",
    "status, attempt, max_attempts, result, error, conversation_id, session_key, pending_message, ",
    "claimed_by, harness_session_ids, created_at, started_at, last_heartbeat_at, completed_at, ",
    "duration_ms, eval, eval_attempt FROM rh_tasks WHERE id = $1"
);

const MARK_RUNNING: &str = concat!(
    "WITH previous AS (",
    "SELECT id, pending_message FROM rh_tasks ",
    "WHERE id = $1 AND status IN ('queued', 'awaiting-input') ",
    "AND (node_affinity IS NULL OR node_affinity = $4) FOR UPDATE",
    "), updated AS (",
    "UPDATE rh_tasks AS tasks SET status = 'running', attempt = tasks.attempt + 1, ",
    "started_at = $2, claimed_by = $3, last_heartbeat_at = $2, pending_message = NULL ",
    "FROM previous WHERE tasks.id = previous.id RETURNING ",
    "tasks.id, tasks.goal, tasks.context_refs, tasks.acceptance_criteria, tasks.spec, ",
    "tasks.executor, tasks.executor_target, tasks.requested_by, tasks.agent_id, tasks.origin, ",
    "tasks.parent_task_id, tasks.chain_depth, tasks.budget, tasks.node_affinity, tasks.usage, ",
    "tasks.status, tasks.attempt, tasks.max_attempts, tasks.result, tasks.error, ",
    "tasks.conversation_id, tasks.session_key, previous.pending_message AS pending_message, ",
    "tasks.claimed_by, tasks.harness_session_ids, tasks.created_at, tasks.started_at, ",
    "tasks.last_heartbeat_at, tasks.completed_at, tasks.duration_ms, tasks.eval, tasks.eval_attempt",
    ") SELECT * FROM updated"
);

const RELEASE_RUNNING: &str = "UPDATE rh_tasks SET status = 'queued', \
     attempt = GREATEST(attempt - 1, 0), claimed_by = NULL, started_at = NULL \
     WHERE id = $1 AND status = 'running'";

pub(crate) async fn insert_task(
    conn: &mut PgConnection,
    task: &TaskRecord,
) -> Result<(), TaskError> {
    let query = "INSERT INTO rh_tasks (
        id, goal, context_refs, acceptance_criteria, spec, executor, executor_target,
        requested_by, agent_id, origin, parent_task_id, chain_depth, budget, node_affinity,
        session_key, pending_message, max_attempts, status
    ) VALUES (
        $1, $2, $3, $4, $5::jsonb, $6, $7, $8, $9, $10, $11, $12, $13::jsonb, $14, $15, $16, $17, $18
    )";
    sqlx::query(query)
        .bind(task.id)
        .bind(&task.goal)
        .bind(Json(&task.context_refs))
        .bind(Json(&task.acceptance_criteria))
        .bind(stored_json(&kernel::task_spec::overrides(&task.spec)?)?)
        .bind(task.executor.to_string())
        .bind(&task.executor_target)
        .bind(&task.requested_by)
        .bind(&task.agent_id)
        .bind(task.origin.to_string())
        .bind(task.parent_task_id)
        .bind(i32_in(task.chain_depth)?)
        .bind(stored_json(&task.budget)?)
        .bind(&task.node_affinity)
        .bind(&task.session_key)
        .bind(&task.pending_message)
        .bind(i32_in(task.max_attempts)?)
        .bind(task.status.to_string())
        .execute(&mut *conn)
        .await
        .map_err(|source| map_sql("insert task", source))?;
    Ok(())
}

pub(crate) async fn load_task(conn: &mut PgConnection, id: Uuid) -> Result<TaskRecord, TaskError> {
    let row = sqlx::query(LOAD_TASK)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|source| map_sql("load task", source))?;
    match row {
        Some(row) => decode_task(&row, "load task"),
        None => Err(TaskError::Missing { id }),
    }
}

pub(crate) async fn mark_running(
    conn: &mut PgConnection,
    id: Uuid,
    claimed_by: &str,
    node: &str,
    now: DateTime<Utc>,
) -> Result<TaskRecord, TaskError> {
    let row = sqlx::query(MARK_RUNNING)
        .bind(id)
        .bind(now)
        .bind(claimed_by)
        .bind(node)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|source| map_sql("mark running", source))?;
    match row {
        Some(row) => decode_task(&row, "mark running"),
        None => conflict_or_missing(conn, id, ConflictAction::Run).await,
    }
}

pub(crate) async fn release_running(conn: &mut PgConnection, id: Uuid) -> Result<(), TaskError> {
    let released = sqlx::query(RELEASE_RUNNING)
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(|source| map_sql("release running", source))?;
    if released.rows_affected() == 0 {
        return conflict_or_missing(conn, id, ConflictAction::Release).await;
    }
    Ok(())
}

pub(crate) async fn finish_task(
    conn: &mut PgConnection,
    id: Uuid,
    finish: &Finish,
) -> Result<bool, TaskError> {
    let result_json = match &finish.result {
        Some(result) => Some(stored_json(result)?),
        None => None,
    };
    let usage_json = match finish.usage {
        Some(usage) => Some(stored_json(&usage)?),
        None => None,
    };
    let result = sqlx::query(
        "UPDATE rh_tasks SET status = $2, result = $3::jsonb, error = $4, usage = $5::jsonb, \
         completed_at = $6, duration_ms = $7, pending_message = NULL \
         WHERE id = $1 AND status <> 'killed'",
    )
    .bind(id)
    .bind(finish.status.to_string())
    .bind(result_json)
    .bind(&finish.error)
    .bind(usage_json)
    .bind(finish.completed_at)
    .bind(finish.duration_ms)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("finish task", source))?;
    if result.rows_affected() == 0 {
        let task = load_task(conn, id).await?;
        if task.status == TaskStatus::Killed {
            return Ok(false);
        }
        return Err(TaskError::Missing { id });
    }
    Ok(true)
}

pub(crate) async fn kill_queued_session(
    conn: &mut PgConnection,
    session_key: &str,
    now: DateTime<Utc>,
) -> Result<(), TaskError> {
    sqlx::query(
        "UPDATE rh_tasks SET status = 'killed', completed_at = $2, \
         error = 'replaced by a newer heartbeat' WHERE session_key = $1 AND status = 'queued'",
    )
    .bind(session_key)
    .bind(now)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("kill queued session", source))?;
    Ok(())
}

pub(crate) async fn kill_task(
    conn: &mut PgConnection,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<TaskStatus, TaskError> {
    let row = sqlx::query(
        "WITH prior AS (SELECT id, status, error FROM rh_tasks WHERE id = $1)
         UPDATE rh_tasks AS task SET status = 'killed', completed_at = $2, \
         error = COALESCE(prior.error, 'Killed by parent')
         FROM prior WHERE task.id = prior.id
           AND prior.status IN ('queued', 'running', 'awaiting-input')
         RETURNING prior.status AS status",
    )
    .bind(id)
    .bind(now)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|source| map_sql("kill task", source))?;
    match row {
        Some(row) => {
            let status: String = cell(&row, "kill task", "status")?;
            parse_text(&status)
        }
        None => conflict_or_missing(conn, id, ConflictAction::Kill).await,
    }
}

pub(crate) async fn set_pending(
    conn: &mut PgConnection,
    id: Uuid,
    message: &str,
) -> Result<(), TaskError> {
    let result = sqlx::query(
        "UPDATE rh_tasks SET pending_message = $2 WHERE id = $1 \
         AND status IN ('queued', 'running', 'awaiting-input')",
    )
    .bind(id)
    .bind(message)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("set pending", source))?;
    if result.rows_affected() == 0 {
        return conflict_or_missing(conn, id, ConflictAction::Steer).await;
    }
    Ok(())
}

pub(crate) async fn add_harness_session(
    conn: &mut PgConnection,
    id: Uuid,
    session: &str,
) -> Result<(), TaskError> {
    let result = sqlx::query(
        "UPDATE rh_tasks SET harness_session_ids = harness_session_ids || jsonb_build_array($2::text) \
         WHERE id = $1",
    )
    .bind(id)
    .bind(session)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("add harness session", source))?;
    if result.rows_affected() == 0 {
        return Err(TaskError::Missing { id });
    }
    Ok(())
}

pub(crate) async fn sweep_tasks(
    conn: &mut PgConnection,
    worker: &str,
    now: DateTime<Utc>,
) -> Result<(), TaskError> {
    sqlx::query(
        "UPDATE rh_tasks SET \
            status = CASE WHEN attempt < max_attempts THEN 'queued' ELSE 'failed' END, \
            claimed_by = CASE WHEN attempt < max_attempts THEN NULL ELSE claimed_by END, \
            completed_at = CASE WHEN attempt < max_attempts THEN NULL ELSE $2 END, \
            error = CASE WHEN attempt < max_attempts THEN error ELSE 'worker_restarted' END \
         WHERE status = 'running' AND claimed_by = $1 \
           AND COALESCE(last_heartbeat_at, started_at) < $2 - interval '90 seconds'",
    )
    .bind(worker)
    .bind(now)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("sweep running", source))?;
    sqlx::query(
        "UPDATE rh_tasks SET status = 'timeout', error = 'awaiting-input expired', completed_at = $1 \
         WHERE status = 'awaiting-input' \
           AND COALESCE(last_heartbeat_at, started_at, created_at) < $1 - \
               (COALESCE((budget->>'maxWallClockMs')::bigint, 86400000) * interval '1 millisecond')",
    )
    .bind(now)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("sweep awaiting", source))?;
    Ok(())
}

async fn conflict_or_missing<T>(
    conn: &mut PgConnection,
    id: Uuid,
    action: ConflictAction,
) -> Result<T, TaskError> {
    let task = load_task(conn, id).await?;
    Err(TaskError::Conflict {
        id,
        status: task.status,
        action,
    })
}

pub(crate) fn decode_task(
    row: &sqlx::postgres::PgRow,
    operation: &'static str,
) -> Result<TaskRecord, TaskError> {
    let id: Uuid = cell(row, operation, "id")?;
    let goal: String = cell(row, operation, "goal")?;
    let agent_id: String = cell(row, operation, "agent_id")?;
    let context_refs: Json<Vec<ContextRef>> = cell(row, operation, "context_refs")?;
    let acceptance_criteria: Json<Vec<AcceptanceCriterion>> =
        cell(row, operation, "acceptance_criteria")?;
    let overrides: Json<TaskOverrides> = cell(row, operation, "spec")?;
    let budget: Json<TaskBudget> = cell(row, operation, "budget")?;
    let session_key: Option<String> = cell(row, operation, "session_key")?;
    let spec = kernel::task_spec::assemble_spec(
        &kernel::task_spec::LoadedColumns {
            id,
            agent_id: &agent_id,
            goal: &goal,
            resolved_context: &context_refs.0,
            acceptance_criteria: &acceptance_criteria.0,
            budget: budget.0,
            session_key: session_key.as_deref(),
        },
        overrides.0,
    )?;
    let executor: String = cell(row, operation, "executor")?;
    let origin: String = cell(row, operation, "origin")?;
    let chain_depth: i32 = cell(row, operation, "chain_depth")?;
    let usage: Option<Json<TaskUsage>> = cell(row, operation, "usage")?;
    let status: String = cell(row, operation, "status")?;
    let attempt: i32 = cell(row, operation, "attempt")?;
    let max_attempts: i32 = cell(row, operation, "max_attempts")?;
    let result: Option<Json<TaskResult>> = cell(row, operation, "result")?;
    let harness_session_ids: Json<Vec<String>> = cell(row, operation, "harness_session_ids")?;
    let eval: Option<Json<EvalOutcome>> = cell(row, operation, "eval")?;
    let eval_attempt: i32 = cell(row, operation, "eval_attempt")?;
    Ok(TaskRecord {
        id,
        goal,
        context_refs: context_refs.0,
        acceptance_criteria: acceptance_criteria.0,
        spec,
        executor: parse_text(&executor)?,
        executor_target: cell(row, operation, "executor_target")?,
        requested_by: cell(row, operation, "requested_by")?,
        agent_id,
        origin: parse_text(&origin)?,
        parent_task_id: cell(row, operation, "parent_task_id")?,
        chain_depth: u32_from(chain_depth)?,
        budget: budget.0,
        node_affinity: cell(row, operation, "node_affinity")?,
        usage: usage.map(|usage| usage.0),
        status: parse_text(&status)?,
        attempt: u32_from(attempt)?,
        max_attempts: u32_from(max_attempts)?,
        result: result.map(|result| result.0),
        error: cell(row, operation, "error")?,
        conversation_id: cell(row, operation, "conversation_id")?,
        session_key,
        pending_message: cell(row, operation, "pending_message")?,
        claimed_by: cell(row, operation, "claimed_by")?,
        harness_session_ids: harness_session_ids.0,
        created_at: cell(row, operation, "created_at")?,
        started_at: cell(row, operation, "started_at")?,
        last_heartbeat_at: cell(row, operation, "last_heartbeat_at")?,
        completed_at: cell(row, operation, "completed_at")?,
        duration_ms: cell(row, operation, "duration_ms")?,
        eval: eval.map(|eval| eval.0),
        eval_attempt: u32_from(eval_attempt)?,
    })
}

const APPEND_PERMISSION: &str = concat!(
    "WITH target AS (",
    "SELECT id, jsonb_typeof(spec) AS spec_type, ",
    "CASE WHEN spec->'permissionDecisions' IS NULL THEN 'absent' ",
    "ELSE jsonb_typeof(spec->'permissionDecisions') END AS decisions_type ",
    "FROM rh_tasks WHERE id = $1",
    "), updated AS (",
    "UPDATE rh_tasks AS tasks SET spec = jsonb_set(",
    "tasks.spec, '{permissionDecisions}', ",
    "COALESCE(tasks.spec->'permissionDecisions', '[]'::jsonb) || jsonb_build_array($2::jsonb), ",
    "true) FROM target WHERE tasks.id = target.id ",
    "AND target.spec_type = 'object' ",
    "AND target.decisions_type IN ('absent', 'array') ",
    "RETURNING tasks.id",
    ") SELECT target.spec_type, target.decisions_type, ",
    "updated.id IS NOT NULL AS appended FROM target ",
    "LEFT JOIN updated ON updated.id = target.id"
);

pub(crate) async fn append_permission(
    conn: &mut PgConnection,
    id: Uuid,
    decision: &TaskPermissionDecision,
) -> Result<(), TaskError> {
    let encoded = stored_json(decision)?;
    let row = sqlx::query(APPEND_PERMISSION)
        .bind(id)
        .bind(encoded)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|source| map_sql("append permission", source))?;
    let Some(row) = row else {
        return Err(TaskError::Missing { id });
    };
    let appended: bool = cell(&row, "append permission", "appended")?;
    if appended {
        return Ok(());
    }
    let spec_type: String = cell(&row, "append permission", "spec_type")?;
    let decisions_type: String = cell(&row, "append permission", "decisions_type")?;
    Err(TaskError::Stored(format!(
        "permissionDecisions cannot be appended (spec {spec_type}, decisions {decisions_type})"
    )))
}

fn stored_json<T: Serialize>(value: &T) -> Result<String, TaskError> {
    serde_json::to_string(value).map_err(|source| TaskError::Stored(source.to_string()))
}

fn i32_in(value: u32) -> Result<i32, TaskError> {
    i32::try_from(value).map_err(|_| TaskError::Stored("integer overflow".to_string()))
}

fn u32_from(value: i32) -> Result<u32, TaskError> {
    u32::try_from(value).map_err(|_| TaskError::Stored("negative integer".to_string()))
}

fn parse_text<T: std::str::FromStr>(text: &str) -> Result<T, TaskError>
where
    T::Err: std::error::Error + Send + Sync + 'static,
{
    text.parse().map_err(|source: T::Err| TaskError::Operation {
        operation: "parse stored task enum",
        source: Box::new(source),
    })
}
