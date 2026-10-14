use chrono::DateTime;
use chrono::Utc;
use serde::Serialize;
use sqlx::PgConnection;
use sqlx::types::Json;
use uuid::Uuid;
use wire::task::AcceptanceCriterion;
use wire::task::ContextRef;
use wire::task::CriterionReport;
use wire::task::Effort;
use wire::task::TaskArtifact;
use wire::task::TaskBudget;
use wire::task::TaskIsolation;
use wire::task::TaskPermissionDecision;
use wire::task::TaskPromptMode;
use wire::task::TaskResult;
use wire::task::TaskSpec;
use wire::task::TaskStatus;
use wire::task::TaskUsage;
use wire::task::TaskVerdict;
use wire::task::VerifierResult;

use crate::error::ConflictAction;
use crate::error::Error;
use crate::payload::as_text;
use crate::payload::from_text;
use crate::record::Finish;
use crate::record::TaskRecord;
use crate::sql_job::cell;
use crate::sql_job::map_sql;

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

pub(crate) async fn insert_task(conn: &mut PgConnection, task: &TaskRecord) -> Result<(), Error> {
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
        .bind(stored_json(&StoredSpec::from(&task.spec))?)
        .bind(as_text(&task.executor)?)
        .bind(&task.executor_target)
        .bind(&task.requested_by)
        .bind(&task.agent_id)
        .bind(as_text(&task.origin)?)
        .bind(task.parent_task_id)
        .bind(i32_in(task.chain_depth)?)
        .bind(stored_json(&StoredBudget::from(task.budget))?)
        .bind(&task.node_affinity)
        .bind(&task.session_key)
        .bind(&task.pending_message)
        .bind(i32_in(task.max_attempts)?)
        .bind(as_text(&task.status)?)
        .execute(&mut *conn)
        .await
        .map_err(|source| map_sql("insert task", source))?;
    Ok(())
}

pub(crate) async fn load_task(conn: &mut PgConnection, id: Uuid) -> Result<TaskRecord, Error> {
    let row = sqlx::query(LOAD_TASK)
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|source| map_sql("load task", source))?;
    match row {
        Some(row) => decode_task(&row, "load task"),
        None => Err(Error::Missing { id }),
    }
}

pub(crate) async fn mark_running(
    conn: &mut PgConnection,
    id: Uuid,
    claimed_by: &str,
    node: &str,
    now: DateTime<Utc>,
) -> Result<TaskRecord, Error> {
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

pub(crate) async fn finish_task(
    conn: &mut PgConnection,
    id: Uuid,
    finish: &Finish,
) -> Result<bool, Error> {
    let result_json = match &finish.result {
        Some(result) => Some(stored_json(&StoredResult::from(result))?),
        None => None,
    };
    let usage_json = match finish.usage {
        Some(usage) => Some(stored_json(&StoredUsage::from(usage))?),
        None => None,
    };
    let result = sqlx::query(
        "UPDATE rh_tasks SET status = $2, result = $3::jsonb, error = $4, usage = $5::jsonb, \
         completed_at = $6, duration_ms = $7, pending_message = NULL \
         WHERE id = $1 AND status <> 'killed'",
    )
    .bind(id)
    .bind(as_text(&finish.status)?)
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
        return Err(Error::Missing { id });
    }
    Ok(true)
}

pub(crate) async fn kill_queued_session(
    conn: &mut PgConnection,
    session_key: &str,
    now: DateTime<Utc>,
) -> Result<(), Error> {
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
) -> Result<TaskStatus, Error> {
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
            from_text(&status)
        }
        None => conflict_or_missing(conn, id, ConflictAction::Kill).await,
    }
}

pub(crate) async fn set_pending(
    conn: &mut PgConnection,
    id: Uuid,
    message: &str,
) -> Result<(), Error> {
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
) -> Result<(), Error> {
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
        return Err(Error::Missing { id });
    }
    Ok(())
}

pub(crate) async fn sweep_tasks(
    conn: &mut PgConnection,
    worker: &str,
    now: DateTime<Utc>,
) -> Result<(), Error> {
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
) -> Result<T, Error> {
    let task = load_task(conn, id).await?;
    Err(Error::Conflict {
        id,
        status: task.status,
        action,
    })
}

fn decode_task(row: &sqlx::postgres::PgRow, operation: &'static str) -> Result<TaskRecord, Error> {
    let context_refs: Json<Vec<ContextRef>> = cell(row, operation, "context_refs")?;
    let acceptance_criteria: Json<Vec<AcceptanceCriterion>> =
        cell(row, operation, "acceptance_criteria")?;
    let spec: Json<TaskSpec> = cell(row, operation, "spec")?;
    let executor: String = cell(row, operation, "executor")?;
    let origin: String = cell(row, operation, "origin")?;
    let chain_depth: i32 = cell(row, operation, "chain_depth")?;
    let budget: Json<TaskBudget> = cell(row, operation, "budget")?;
    let usage: Option<Json<TaskUsage>> = cell(row, operation, "usage")?;
    let status: String = cell(row, operation, "status")?;
    let attempt: i32 = cell(row, operation, "attempt")?;
    let max_attempts: i32 = cell(row, operation, "max_attempts")?;
    let result: Option<Json<TaskResult>> = cell(row, operation, "result")?;
    let harness_session_ids: Json<Vec<String>> = cell(row, operation, "harness_session_ids")?;
    let eval: Option<Json<VerifierResult>> = cell(row, operation, "eval")?;
    let eval_attempt: i32 = cell(row, operation, "eval_attempt")?;
    Ok(TaskRecord {
        id: cell(row, operation, "id")?,
        goal: cell(row, operation, "goal")?,
        context_refs: context_refs.0,
        acceptance_criteria: acceptance_criteria.0,
        spec: spec.0,
        executor: from_text(&executor)?,
        executor_target: cell(row, operation, "executor_target")?,
        requested_by: cell(row, operation, "requested_by")?,
        agent_id: cell(row, operation, "agent_id")?,
        origin: from_text(&origin)?,
        parent_task_id: cell(row, operation, "parent_task_id")?,
        chain_depth: u32_from(chain_depth)?,
        budget: budget.0,
        node_affinity: cell(row, operation, "node_affinity")?,
        usage: usage.map(|usage| usage.0),
        status: from_text(&status)?,
        attempt: u32_from(attempt)?,
        max_attempts: u32_from(max_attempts)?,
        result: result.map(|result| result.0),
        error: cell(row, operation, "error")?,
        conversation_id: cell(row, operation, "conversation_id")?,
        session_key: cell(row, operation, "session_key")?,
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

fn stored_json<T: Serialize>(value: &T) -> Result<String, Error> {
    serde_json::to_string(value).map_err(|source| Error::Stored(source.to_string()))
}

#[derive(Serialize)]
struct StoredBudget {
    #[serde(rename = "maxUsd")]
    usd: Option<f64>,
    #[serde(rename = "maxTokens")]
    tokens: Option<u64>,
    #[serde(rename = "maxTurns")]
    turns: Option<u32>,
    #[serde(rename = "maxWallClockMs")]
    wall_clock_ms: Option<u64>,
}

impl From<TaskBudget> for StoredBudget {
    fn from(budget: TaskBudget) -> Self {
        Self {
            usd: budget.max_usd,
            tokens: budget.max_tokens,
            turns: budget.max_turns,
            wall_clock_ms: budget.max_wall_clock_ms,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredUsage {
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    cost_usd: Option<f64>,
    turns: u32,
    wall_clock_ms: u64,
}

impl From<TaskUsage> for StoredUsage {
    fn from(usage: TaskUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            total_tokens: usage.total_tokens,
            cost_usd: usage.cost_usd,
            turns: usage.turns,
            wall_clock_ms: usage.wall_clock_ms,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredResult<'a> {
    verdict: TaskVerdict,
    summary: &'a str,
    output: Option<&'a str>,
    artifacts: &'a [TaskArtifact],
    criteria_self_report: Option<&'a [CriterionReport]>,
    usage: StoredUsage,
    error: Option<&'a str>,
}

impl<'a> From<&'a TaskResult> for StoredResult<'a> {
    fn from(result: &'a TaskResult) -> Self {
        Self {
            verdict: result.verdict,
            summary: &result.summary,
            output: result.output.as_deref(),
            artifacts: &result.artifacts,
            criteria_self_report: result.criteria_self_report.as_deref(),
            usage: StoredUsage::from(result.usage),
            error: result.error.as_deref(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StoredSpec<'a> {
    task_id: &'a str,
    agent_id: &'a str,
    goal: &'a str,
    resolved_context: &'a [ContextRef],
    acceptance_criteria: &'a [AcceptanceCriterion],
    budget: StoredBudget,
    tools: Option<&'a [String]>,
    working_dir: Option<&'a str>,
    model: Option<&'a str>,
    effort: Option<Effort>,
    system_prompt_append: Option<&'a str>,
    isolation: Option<TaskIsolation>,
    prompt_mode: Option<TaskPromptMode>,
    exclude_tools: Option<&'a [String]>,
    resume_message: Option<&'a str>,
    session: &'a str,
    permission_decisions: Option<&'a [TaskPermissionDecision]>,
}

impl<'a> From<&'a TaskSpec> for StoredSpec<'a> {
    fn from(spec: &'a TaskSpec) -> Self {
        Self {
            task_id: &spec.task_id,
            agent_id: &spec.agent_id,
            goal: &spec.goal,
            resolved_context: &spec.resolved_context,
            acceptance_criteria: &spec.acceptance_criteria,
            budget: StoredBudget::from(spec.budget),
            tools: spec.tools.as_deref(),
            working_dir: spec.working_dir.as_deref(),
            model: spec.model.as_deref(),
            effort: spec.effort,
            system_prompt_append: spec.system_prompt_append.as_deref(),
            isolation: spec.isolation,
            prompt_mode: spec.prompt_mode,
            exclude_tools: spec.exclude_tools.as_deref(),
            resume_message: spec.resume_message.as_deref(),
            session: &spec.session,
            permission_decisions: spec.permission_decisions.as_deref(),
        }
    }
}

fn i32_in(value: u32) -> Result<i32, Error> {
    i32::try_from(value).map_err(|_| Error::Stored("integer overflow".to_string()))
}

fn u32_from(value: i32) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| Error::Stored("negative integer".to_string()))
}
