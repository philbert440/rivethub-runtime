use chrono::DateTime;
use chrono::Utc;
use kernel::tasks::ConflictAction;
use kernel::tasks::Finish;
use kernel::tasks::TaskError;
use kernel::tasks::TaskRecord;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::Transaction;
use rusqlite::params;
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;
use wire::task::TaskStatus;

mod decode;
mod list;
mod permission;
mod sweep;

use decode::{map_row, record_from};
pub(crate) use list::list_tasks;
pub(crate) use permission::append_permission;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS rh_tasks (
    id TEXT PRIMARY KEY,
    goal TEXT NOT NULL,
    context_refs TEXT NOT NULL,
    acceptance_criteria TEXT NOT NULL,
    spec TEXT NOT NULL,
    executor TEXT NOT NULL CHECK (executor IN ('chat-loop','harness-session','mesh')),
    executor_target TEXT,
    requested_by TEXT,
    agent_id TEXT NOT NULL,
    origin TEXT NOT NULL CHECK (origin IN ('heartbeat','chat','tool','mesh','api','eval')),
    parent_task_id TEXT REFERENCES rh_tasks(id) ON DELETE SET NULL,
    chain_depth INTEGER NOT NULL,
    budget TEXT NOT NULL,
    node_affinity TEXT,
    usage TEXT,
    status TEXT NOT NULL CHECK (status IN ('queued','running','awaiting-input','completed','failed','killed','timeout')),
    attempt INTEGER NOT NULL,
    max_attempts INTEGER NOT NULL,
    result TEXT,
    error TEXT,
    conversation_id TEXT,
    session_key TEXT,
    pending_message TEXT,
    claimed_by TEXT,
    harness_session_ids TEXT NOT NULL,
    created_at TEXT NOT NULL,
    started_at TEXT,
    last_heartbeat_at TEXT,
    completed_at TEXT,
    duration_ms INTEGER,
    eval TEXT,
    eval_attempt INTEGER NOT NULL
);
CREATE TEMP TRIGGER IF NOT EXISTS rh_task_done AFTER UPDATE OF status ON main.rh_tasks
WHEN NEW.status IN ('completed','failed','killed','timeout')
    AND OLD.status NOT IN ('completed','failed','killed','timeout')
BEGIN
    SELECT rh_task_done(NEW.id);
END;";

const LOAD: &str =
    "SELECT id, goal, context_refs, acceptance_criteria, spec, executor, executor_target,
    requested_by, agent_id, origin, parent_task_id, chain_depth, budget, node_affinity, usage,
    status, attempt, max_attempts, result, error, conversation_id, session_key, pending_message,
    claimed_by, harness_session_ids, created_at, started_at, last_heartbeat_at, completed_at,
    duration_ms, eval, eval_attempt FROM rh_tasks WHERE id = ?1";

pub(crate) fn insert_task(conn: &mut Connection, task: &TaskRecord) -> Result<(), TaskError> {
    let tx = immediate(conn, "insert task")?;
    ensure(&tx, "insert task")?;
    write_insert(&tx, task)?;
    tx.commit()
        .map_err(|source| sql_error("insert task", source))?;
    Ok(())
}

pub(crate) fn load_task(conn: &Connection, id: Uuid) -> Result<TaskRecord, TaskError> {
    ensure(conn, "load task")?;
    load_required(conn, id, "load task")
}

pub(crate) fn mark_running(
    conn: &mut Connection,
    id: Uuid,
    claimed_by: &str,
    node: &str,
    now: DateTime<Utc>,
) -> Result<TaskRecord, TaskError> {
    let tx = immediate(conn, "mark running")?;
    ensure(&tx, "mark running")?;
    let record = claim_running(&tx, id, claimed_by, node, now)?;
    tx.commit()
        .map_err(|source| sql_error("mark running", source))?;
    Ok(record)
}

pub(crate) fn release_running(conn: &mut Connection, id: Uuid) -> Result<(), TaskError> {
    let tx = immediate(conn, "release task")?;
    ensure(&tx, "release task")?;
    let Some(task) = load_optional(&tx, id, "release task")? else {
        return Err(TaskError::Missing { id });
    };
    if task.status != TaskStatus::Running {
        return Err(TaskError::Conflict {
            id,
            status: task.status,
            action: ConflictAction::Release,
        });
    }
    let attempt = task
        .attempt
        .checked_sub(1)
        .ok_or_else(|| TaskError::Stored("attempt underflow".to_owned()))?;
    let rows = tx
        .execute(
            "UPDATE rh_tasks SET status = 'queued', attempt = ?2, claimed_by = NULL, started_at = NULL
             WHERE id = ?1 AND status = 'running'",
            params![id.to_string(), i64::from(attempt)],
        )
        .map_err(|source| sql_error("release task", source))?;
    if rows != 1 {
        return Err(TaskError::Stored(
            "release running missed the row".to_owned(),
        ));
    }
    tx.commit()
        .map_err(|source| sql_error("release task", source))?;
    Ok(())
}

pub(crate) fn finish_task(
    conn: &mut Connection,
    id: Uuid,
    finish: &Finish,
) -> Result<bool, TaskError> {
    let tx = immediate(conn, "finish task")?;
    ensure(&tx, "finish task")?;
    let finished = write_finish(&tx, id, finish)?;
    tx.commit()
        .map_err(|source| sql_error("finish task", source))?;
    Ok(finished)
}

pub(crate) fn sweep_tasks(
    conn: &mut Connection,
    worker: &str,
    now: DateTime<Utc>,
) -> Result<(), TaskError> {
    let tx = immediate(conn, "sweep tasks")?;
    ensure(&tx, "sweep tasks")?;
    sweep::running(&tx, worker, now)?;
    sweep::awaiting(&tx, now)?;
    tx.commit()
        .map_err(|source| sql_error("sweep tasks", source))?;
    Ok(())
}

pub(crate) fn heartbeat(
    conn: &Connection,
    id: Uuid,
    worker: &str,
    now: DateTime<Utc>,
) -> Result<(), TaskError> {
    conn.execute(
        "UPDATE rh_tasks SET last_heartbeat_at = ?3
         WHERE id = ?1 AND claimed_by = ?2 AND status IN ('running', 'awaiting-input')",
        params![id.to_string(), worker, stamp(now)],
    )
    .map_err(|source| sql_error("task heartbeat", source))?;
    Ok(())
}

pub(crate) fn kill_queued(
    conn: &mut Connection,
    session_key: &str,
    now: DateTime<Utc>,
) -> Result<(), TaskError> {
    let tx = immediate(conn, "kill queued session")?;
    ensure(&tx, "kill queued session")?;
    tx.execute(
        "UPDATE rh_tasks SET status = 'killed', completed_at = ?2,
            error = 'replaced by a newer heartbeat' WHERE session_key = ?1 AND status = 'queued'",
        params![session_key, stamp(now)],
    )
    .map_err(|source| sql_error("kill queued session", source))?;
    tx.commit()
        .map_err(|source| sql_error("kill queued session", source))?;
    Ok(())
}

pub(crate) fn kill_task(
    conn: &mut Connection,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<TaskStatus, TaskError> {
    let tx = immediate(conn, "kill task")?;
    ensure(&tx, "kill task")?;
    let status = write_kill(&tx, id, now)?;
    tx.commit()
        .map_err(|source| sql_error("kill task", source))?;
    Ok(status)
}

pub(crate) fn set_pending(conn: &mut Connection, id: Uuid, message: &str) -> Result<(), TaskError> {
    let tx = immediate(conn, "set pending")?;
    ensure(&tx, "set pending")?;
    write_pending(&tx, id, message)?;
    tx.commit()
        .map_err(|source| sql_error("set pending", source))?;
    Ok(())
}

pub(crate) fn add_harness(conn: &mut Connection, id: Uuid, session: &str) -> Result<(), TaskError> {
    let tx = immediate(conn, "add harness session")?;
    ensure(&tx, "add harness session")?;
    write_harness(&tx, id, session)?;
    tx.commit()
        .map_err(|source| sql_error("add harness session", source))?;
    Ok(())
}

fn write_insert(conn: &Connection, task: &TaskRecord) -> Result<(), TaskError> {
    conn.execute(
        "INSERT INTO rh_tasks (
            id, goal, context_refs, acceptance_criteria, spec, executor, executor_target,
            requested_by, agent_id, origin, parent_task_id, chain_depth, budget, node_affinity,
            usage, status, attempt, max_attempts, result, error, conversation_id, session_key,
            pending_message, claimed_by, harness_session_ids, created_at, started_at,
            last_heartbeat_at, completed_at, duration_ms, eval, eval_attempt
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
            ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32
         )",
        params![
            task.id.to_string(),
            task.goal,
            encode(&task.context_refs)?,
            encode(&task.acceptance_criteria)?,
            encode(&kernel::task_spec::overrides(&task.spec)?)?,
            task.executor.to_string(),
            task.executor_target,
            task.requested_by,
            task.agent_id,
            task.origin.to_string(),
            uuid_text(task.parent_task_id),
            i64::from(task.chain_depth),
            encode(&task.budget)?,
            task.node_affinity,
            encode_opt(task.usage)?,
            task.status.to_string(),
            i64::from(task.attempt),
            i64::from(task.max_attempts),
            encode_opt(task.result.clone())?,
            task.error,
            uuid_text(task.conversation_id),
            task.session_key,
            task.pending_message,
            task.claimed_by,
            encode(&task.harness_session_ids)?,
            stamp(task.created_at),
            stamp_opt(task.started_at),
            stamp_opt(task.last_heartbeat_at),
            stamp_opt(task.completed_at),
            task.duration_ms,
            encode_opt(task.eval.clone())?,
            i64::from(task.eval_attempt)
        ],
    )
    .map_err(|source| sql_error("insert task", source))?;
    Ok(())
}

fn claim_running(
    conn: &Connection,
    id: Uuid,
    claimed_by: &str,
    node: &str,
    now: DateTime<Utc>,
) -> Result<TaskRecord, TaskError> {
    let Some(mut task) = load_optional(conn, id, "mark running")? else {
        return Err(TaskError::Missing { id });
    };
    if !can_run(&task, node) {
        return Err(TaskError::Conflict {
            id,
            status: task.status,
            action: ConflictAction::Run,
        });
    }
    let attempt = task
        .attempt
        .checked_add(1)
        .ok_or_else(|| TaskError::Stored("integer overflow".to_owned()))?;
    let rows = conn
        .execute(
            "UPDATE rh_tasks SET status = 'running', attempt = ?2, started_at = ?3,
                claimed_by = ?4, last_heartbeat_at = ?3, pending_message = NULL
             WHERE id = ?1 AND status IN ('queued', 'awaiting-input')",
            params![id.to_string(), i64::from(attempt), stamp(now), claimed_by],
        )
        .map_err(|source| sql_error("mark running", source))?;
    if rows != 1 {
        return Err(TaskError::Stored("mark running missed the row".to_owned()));
    }
    task.status = TaskStatus::Running;
    task.attempt = attempt;
    task.started_at = Some(now);
    task.last_heartbeat_at = Some(now);
    task.claimed_by = Some(claimed_by.to_owned());
    Ok(task)
}

fn can_run(task: &TaskRecord, node: &str) -> bool {
    let open = matches!(task.status, TaskStatus::Queued | TaskStatus::AwaitingInput);
    open && task
        .node_affinity
        .as_deref()
        .is_none_or(|affinity| affinity == node)
}

fn write_finish(conn: &Connection, id: Uuid, finish: &Finish) -> Result<bool, TaskError> {
    let rows = conn
        .execute(
            "UPDATE rh_tasks SET status = ?2, result = ?3, error = ?4, usage = ?5,
                completed_at = ?6, duration_ms = ?7, pending_message = NULL
             WHERE id = ?1 AND status <> 'killed'",
            params![
                id.to_string(),
                finish.status.to_string(),
                encode_opt(finish.result.clone())?,
                finish.error,
                encode_opt(finish.usage)?,
                stamp(finish.completed_at),
                finish.duration_ms
            ],
        )
        .map_err(|source| sql_error("finish task", source))?;
    if rows == 1 {
        return Ok(true);
    }
    match load_optional(conn, id, "finish task")? {
        Some(task) if task.status == TaskStatus::Killed => Ok(false),
        Some(_) | None => Err(TaskError::Missing { id }),
    }
}

fn write_kill(conn: &Connection, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, TaskError> {
    let Some(task) = load_optional(conn, id, "kill task")? else {
        return Err(TaskError::Missing { id });
    };
    if !live(task.status) {
        return Err(TaskError::Conflict {
            id,
            status: task.status,
            action: ConflictAction::Kill,
        });
    }
    let error = match task.error {
        Some(existing) => existing,
        None => "Killed by parent".to_owned(),
    };
    let rows = conn
        .execute(
            "UPDATE rh_tasks SET status = 'killed', completed_at = ?2, error = ?3
             WHERE id = ?1 AND status IN ('queued', 'running', 'awaiting-input')",
            params![id.to_string(), stamp(now), error],
        )
        .map_err(|source| sql_error("kill task", source))?;
    if rows == 1 {
        Ok(task.status)
    } else {
        Err(TaskError::Stored("kill missed the row".to_owned()))
    }
}

fn write_pending(conn: &Connection, id: Uuid, message: &str) -> Result<(), TaskError> {
    let Some(task) = load_optional(conn, id, "set pending")? else {
        return Err(TaskError::Missing { id });
    };
    if !live(task.status) {
        return Err(TaskError::Conflict {
            id,
            status: task.status,
            action: ConflictAction::Steer,
        });
    }
    let rows = conn
        .execute(
            "UPDATE rh_tasks SET pending_message = ?2
             WHERE id = ?1 AND status IN ('queued', 'running', 'awaiting-input')",
            params![id.to_string(), message],
        )
        .map_err(|source| sql_error("set pending", source))?;
    if rows == 1 {
        Ok(())
    } else {
        Err(TaskError::Stored("set pending missed the row".to_owned()))
    }
}

fn write_harness(conn: &Connection, id: Uuid, session: &str) -> Result<(), TaskError> {
    let Some(mut task) = load_optional(conn, id, "add harness session")? else {
        return Err(TaskError::Missing { id });
    };
    task.harness_session_ids.push(session.to_owned());
    let rows = conn
        .execute(
            "UPDATE rh_tasks SET harness_session_ids = ?2 WHERE id = ?1",
            params![id.to_string(), encode(&task.harness_session_ids)?],
        )
        .map_err(|source| sql_error("add harness session", source))?;
    if rows == 1 {
        Ok(())
    } else {
        Err(TaskError::Missing { id })
    }
}

fn load_required(
    conn: &Connection,
    id: Uuid,
    operation: &'static str,
) -> Result<TaskRecord, TaskError> {
    load_optional(conn, id, operation)?.ok_or(TaskError::Missing { id })
}

fn load_optional(
    conn: &Connection,
    id: Uuid,
    operation: &'static str,
) -> Result<Option<TaskRecord>, TaskError> {
    let raw = conn
        .query_row(LOAD, params![id.to_string()], map_row)
        .optional()
        .map_err(|source| sql_error(operation, source))?;
    match raw {
        None => Ok(None),
        Some(raw) => record_from(raw).map(Some),
    }
}

fn live(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Queued | TaskStatus::Running | TaskStatus::AwaitingInput
    )
}

fn immediate<'conn>(
    conn: &'conn mut Connection,
    operation: &'static str,
) -> Result<Transaction<'conn>, TaskError> {
    conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(|source| sql_error(operation, source))
}

fn ensure(conn: &Connection, operation: &'static str) -> Result<(), TaskError> {
    conn.execute_batch(SCHEMA)
        .map_err(|source| sql_error(operation, source))
}

fn encode<T: Serialize>(value: &T) -> Result<String, TaskError> {
    serde_json::to_string(value).map_err(|source| TaskError::Stored(source.to_string()))
}

fn encode_opt<T: Serialize>(value: Option<T>) -> Result<Option<String>, TaskError> {
    match value {
        None => Ok(None),
        Some(value) => encode(&value).map(Some),
    }
}

fn decode<T: DeserializeOwned>(text: &str) -> Result<T, TaskError> {
    serde_json::from_str(text).map_err(|source| TaskError::Stored(source.to_string()))
}

fn decode_opt<T: DeserializeOwned>(text: Option<String>) -> Result<Option<T>, TaskError> {
    match text {
        None => Ok(None),
        Some(text) => decode(&text).map(Some),
    }
}

fn parse_enum<T>(text: &str) -> Result<T, TaskError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    text.parse()
        .map_err(|source: T::Err| TaskError::Stored(source.to_string()))
}

fn parse_uuid(text: &str) -> Result<Uuid, TaskError> {
    Uuid::parse_str(text).map_err(|_| TaskError::Stored(format!("uuid {text}")))
}

fn uuid_opt(value: Option<String>) -> Result<Option<Uuid>, TaskError> {
    match value {
        None => Ok(None),
        Some(text) => parse_uuid(&text).map(Some),
    }
}

fn uuid_text(value: Option<Uuid>) -> Option<String> {
    value.map(|id| id.to_string())
}

fn parse_time(text: &str) -> Result<DateTime<Utc>, TaskError> {
    DateTime::parse_from_rfc3339(text)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| TaskError::Stored(format!("timestamp {text}")))
}

fn time_opt(value: Option<String>) -> Result<Option<DateTime<Utc>>, TaskError> {
    match value {
        None => Ok(None),
        Some(text) => parse_time(&text).map(Some),
    }
}

fn stamp(at: DateTime<Utc>) -> String {
    crate::time::stamp(at)
}

fn stamp_opt(at: Option<DateTime<Utc>>) -> Option<String> {
    at.map(stamp)
}

fn u32_in(value: i64) -> Result<u32, TaskError> {
    u32::try_from(value).map_err(|_| TaskError::Stored("negative integer".to_owned()))
}

fn i32_opt(value: Option<i64>) -> Result<Option<i32>, TaskError> {
    match value {
        None => Ok(None),
        Some(raw) => i32::try_from(raw)
            .map(Some)
            .map_err(|_| TaskError::Stored("duration".to_owned())),
    }
}

fn sql_error(operation: &'static str, source: rusqlite::Error) -> TaskError {
    TaskError::Operation {
        operation,
        source: Box::new(source),
    }
}
