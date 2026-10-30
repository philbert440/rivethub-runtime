use std::str::FromStr;
use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use kernel::tasks::Budget;
use kernel::tasks::Finish;
use kernel::tasks::Harness;
use kernel::tasks::Parent;
use kernel::tasks::ParentSource;
use kernel::tasks::Target;
use kernel::tasks::Task;
use kernel::tasks::TaskError;
use kernel::tasks::TaskFilter;
use kernel::tasks::TaskStatus;

const NOTIFY: &str =
    "CREATE TEMP TRIGGER IF NOT EXISTS rh_task_inserted AFTER INSERT ON main.rh_tasks
BEGIN
    SELECT rh_task_changed(NEW.id);
END;
CREATE TEMP TRIGGER IF NOT EXISTS rh_task_changed AFTER UPDATE OF status ON main.rh_tasks
WHEN NEW.status IS NOT OLD.status
BEGIN
    SELECT rh_task_changed(NEW.id);
END;";

const BY_ID: &str =
    "SELECT id, goal, acceptance_criteria, budget, status, result, error, usage, agent, preset,
            node, harness, model, effort, cwd, permission_mode, parent_session_id, parent_task_id,
            parent_source, depth, child_session_id, requested_by, owner_user_id, created_at,
            started_at, completed_at, alive_at FROM rh_tasks WHERE id = ?1";

const BY_CHILD: &str =
    "SELECT id, goal, acceptance_criteria, budget, status, result, error, usage, agent, preset,
            node, harness, model, effort, cwd, permission_mode, parent_session_id, parent_task_id,
            parent_source, depth, child_session_id, requested_by, owner_user_id, created_at,
            started_at, completed_at, alive_at FROM rh_tasks WHERE child_session_id = ?1";

const LIST: &str =
    "SELECT id, goal, acceptance_criteria, budget, status, result, error, usage, agent, preset,
            node, harness, model, effort, cwd, permission_mode, parent_session_id, parent_task_id,
            parent_source, depth, child_session_id, requested_by, owner_user_id, created_at,
            started_at, completed_at, alive_at FROM rh_tasks WHERE (?1 IS NULL OR status = ?1)
            AND (?2 IS NULL OR agent = ?2) ORDER BY created_at DESC, id DESC LIMIT ?3";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct StoredBudget {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_wall_clock_ms: Option<u64>,
}

pub(crate) fn insert(conn: &Connection, task: &Task) -> Result<(), TaskError> {
    ensure(conn, "insert task")?;
    conn.execute(
        "INSERT INTO rh_tasks (id, goal, acceptance_criteria, budget, status, agent, preset, node,
            harness, model, effort, cwd, permission_mode, parent_session_id, parent_task_id,
            parent_source, depth, requested_by, owner_user_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
            ?18, ?19, ?20)",
        params![
            task.id.to_string(),
            task.goal,
            encode(&task.criteria)?,
            encode(&budget_out(task.budget)?)?,
            task.status.to_string(),
            task.target.agent,
            task.target.preset,
            task.target.node,
            task.target.harness.map(|harness| harness.to_string()),
            task.target.model,
            task.target.effort,
            task.target.cwd,
            task.target.permission_mode,
            task.parent.session,
            task.parent.task.map(|id| id.to_string()),
            task.parent.source.map(|source| source.to_string()),
            i64::from(task.parent.depth),
            task.requested_by,
            task.owner,
            stamp(task.created_at),
        ],
    )
    .map_err(failed("insert task"))?;
    Ok(())
}

pub(crate) fn load(conn: &Connection, id: Uuid) -> Result<Task, TaskError> {
    one(conn, BY_ID, &id.to_string())?.ok_or(TaskError::Missing { id })
}

pub(crate) fn by_child(conn: &Connection, session: &str) -> Result<Option<Task>, TaskError> {
    one(conn, BY_CHILD, session)
}

pub(crate) fn list(conn: &Connection, filter: &TaskFilter) -> Result<Vec<Task>, TaskError> {
    ensure(conn, "list tasks")?;
    let mut statement = conn.prepare(LIST).map_err(failed("list tasks"))?;
    let limit = filter.limit.map_or(-1, i64::from);
    let rows = statement
        .query_map(
            params![
                filter.status.map(|status| status.to_string()),
                filter.agent,
                limit
            ],
            Raw::read,
        )
        .map_err(failed("list tasks"))?;
    let mut tasks = Vec::new();
    for row in rows {
        tasks.push(row.map_err(failed("list tasks"))?.task()?);
    }
    Ok(tasks)
}

pub(crate) fn begin(
    conn: &Connection,
    id: Uuid,
    node: &str,
    now: DateTime<Utc>,
) -> Result<bool, TaskError> {
    ensure(conn, "begin task")?;
    let rows = conn
        .execute(
            "UPDATE rh_tasks SET status = 'running', started_at = ?3, alive_at = ?3
             WHERE id = ?1 AND node = ?2 AND status = 'queued'",
            params![id.to_string(), node, stamp(now)],
        )
        .map_err(failed("begin task"))?;
    Ok(rows == 1)
}

pub(crate) fn attach(
    conn: &Connection,
    id: Uuid,
    child: &str,
    harness: Harness,
) -> Result<(), TaskError> {
    conn.execute(
        "UPDATE rh_tasks SET child_session_id = ?2, harness = ?3 WHERE id = ?1",
        params![id.to_string(), child, harness.to_string()],
    )
    .map_err(failed("attach child"))?;
    Ok(())
}

pub(crate) fn flip(conn: &Connection, id: Uuid, status: TaskStatus) -> Result<bool, TaskError> {
    let from = match status {
        TaskStatus::Running => TaskStatus::AwaitingInput,
        TaskStatus::AwaitingInput => TaskStatus::Running,
        TaskStatus::Queued
        | TaskStatus::Completed
        | TaskStatus::Failed
        | TaskStatus::Killed
        | TaskStatus::Timeout => {
            return Ok(false);
        }
    };
    ensure(conn, "flip task")?;
    let rows = conn
        .execute(
            "UPDATE rh_tasks SET status = ?3 WHERE id = ?1 AND status = ?2",
            params![id.to_string(), from.to_string(), status.to_string()],
        )
        .map_err(failed("flip task"))?;
    Ok(rows == 1)
}

pub(crate) fn alive(conn: &Connection, ids: &[Uuid], now: DateTime<Utc>) -> Result<(), TaskError> {
    for id in ids {
        conn.execute(
            "UPDATE rh_tasks SET alive_at = ?2
             WHERE id = ?1 AND status IN ('running', 'awaiting-input')",
            params![id.to_string(), stamp(now)],
        )
        .map_err(failed("task liveness"))?;
    }
    Ok(())
}

pub(crate) fn finish(conn: &Connection, id: Uuid, finish: &Finish) -> Result<bool, TaskError> {
    ensure(conn, "finish task")?;
    let opened = if finish.status == TaskStatus::Completed {
        "('running', 'awaiting-input')"
    } else {
        "('queued', 'running', 'awaiting-input')"
    };
    let sql = format!(
        "UPDATE rh_tasks SET status = ?2, result = ?3, error = ?4, usage = ?5, completed_at = ?6
         WHERE id = ?1 AND status IN {opened}"
    );
    let rows = conn
        .execute(
            &sql,
            params![
                id.to_string(),
                finish.status.to_string(),
                finish.result.as_ref().map(encode).transpose()?,
                finish.error,
                finish.usage.as_ref().map(encode).transpose()?,
                stamp(finish.at),
            ],
        )
        .map_err(failed("finish task"))?;
    Ok(rows == 1)
}

pub(crate) fn kill(
    conn: &Connection,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<TaskStatus, TaskError> {
    let prior = load(conn, id)?.status;
    if !prior.terminal() {
        conn.execute(
            "UPDATE rh_tasks SET status = 'killed', completed_at = ?2 WHERE id = ?1",
            params![id.to_string(), stamp(now)],
        )
        .map_err(failed("kill task"))?;
    }
    Ok(prior)
}

pub(crate) fn fail_open(
    conn: &Connection,
    filter: &str,
    bound: &str,
    error: &str,
    now: DateTime<Utc>,
) -> Result<Vec<Uuid>, TaskError> {
    ensure(conn, "fail open tasks")?;
    let sql = format!(
        "UPDATE rh_tasks SET status = 'failed', completed_at = ?2, error = ?3
         WHERE status IN ('running', 'awaiting-input') AND {filter} RETURNING id"
    );
    let mut statement = conn.prepare(&sql).map_err(failed("fail open tasks"))?;
    let ids = statement
        .query_map(params![bound, stamp(now), error], |row| {
            row.get::<_, String>(0)
        })
        .map_err(failed("fail open tasks"))?;
    let mut failed_ids = Vec::new();
    for id in ids {
        failed_ids.push(parse_uuid(&id.map_err(failed("fail open tasks"))?)?);
    }
    Ok(failed_ids)
}

pub(crate) fn stamp(at: DateTime<Utc>) -> String {
    crate::time::stamp(at)
}

fn one(conn: &Connection, sql: &str, key: &str) -> Result<Option<Task>, TaskError> {
    ensure(conn, "load task")?;
    conn.query_row(sql, params![key], Raw::read)
        .optional()
        .map_err(failed("load task"))?
        .map(Raw::task)
        .transpose()
}

fn ensure(conn: &Connection, operation: &'static str) -> Result<(), TaskError> {
    conn.execute_batch(NOTIFY).map_err(failed(operation))
}

struct Raw {
    text: [Option<String>; 27],
    depth: i64,
}

impl Raw {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let mut text: [Option<String>; 27] = Default::default();
        for (index, slot) in text.iter_mut().enumerate() {
            if index != 19 {
                *slot = row.get(index)?;
            }
        }
        Ok(Self {
            text,
            depth: row.get(19)?,
        })
    }

    fn take(&mut self, index: usize) -> Option<String> {
        self.text.get_mut(index).and_then(Option::take)
    }

    fn need(&mut self, index: usize) -> Result<String, TaskError> {
        self.take(index)
            .ok_or_else(|| TaskError::Stored(format!("task column {index} is empty")))
    }

    fn task(mut self) -> Result<Task, TaskError> {
        let budget: StoredBudget = decode(&self.need(3)?)?;
        Ok(Task {
            id: parse_uuid(&self.need(0)?)?,
            goal: self.need(1)?,
            criteria: decode(&self.need(2)?)?,
            budget: Budget {
                wall_clock: budget.max_wall_clock_ms.map(Duration::from_millis),
                max_turns: budget.max_turns,
            },
            status: parse(&self.need(4)?)?,
            result: self.take(5).as_deref().map(decode).transpose()?,
            error: self.take(6),
            usage: self.take(7).as_deref().map(decode).transpose()?,
            target: Target {
                agent: self.need(8)?,
                preset: self.take(9),
                node: self.need(10)?,
                harness: self.take(11).as_deref().map(parse::<Harness>).transpose()?,
                model: self.take(12),
                effort: self.take(13),
                cwd: self.take(14),
                permission_mode: self.take(15),
            },
            parent: Parent {
                session: self.take(16),
                task: self.take(17).as_deref().map(parse_uuid).transpose()?,
                source: self
                    .take(18)
                    .as_deref()
                    .map(parse::<ParentSource>)
                    .transpose()?,
                depth: u8::try_from(self.depth)
                    .map_err(|_| TaskError::Stored("task depth".to_owned()))?,
            },
            child: self.take(20),
            requested_by: self.take(21),
            owner: self.need(22)?,
            created_at: time(&self.need(23)?)?,
            started_at: self.take(24).as_deref().map(time).transpose()?,
            completed_at: self.take(25).as_deref().map(time).transpose()?,
            alive_at: self.take(26).as_deref().map(time).transpose()?,
        })
    }
}

fn budget_out(budget: Budget) -> Result<StoredBudget, TaskError> {
    let max_wall_clock_ms = budget
        .wall_clock
        .map(|wall| u64::try_from(wall.as_millis()))
        .transpose()
        .map_err(|_| TaskError::Stored("maxWallClockMs".to_owned()))?;
    Ok(StoredBudget {
        max_turns: budget.max_turns,
        max_wall_clock_ms,
    })
}

fn encode<T: Serialize>(value: &T) -> Result<String, TaskError> {
    serde_json::to_string(value).map_err(|source| TaskError::Stored(source.to_string()))
}

fn decode<T: DeserializeOwned>(text: &str) -> Result<T, TaskError> {
    serde_json::from_str(text).map_err(|source| TaskError::Stored(source.to_string()))
}

fn parse<T: FromStr>(text: &str) -> Result<T, TaskError> {
    text.parse()
        .map_err(|_| TaskError::Stored(format!("task field {text}")))
}

fn parse_uuid(text: &str) -> Result<Uuid, TaskError> {
    Uuid::parse_str(text).map_err(|_| TaskError::Stored(format!("uuid {text}")))
}

fn time(text: &str) -> Result<DateTime<Utc>, TaskError> {
    DateTime::parse_from_rfc3339(text)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|_| TaskError::Stored(format!("timestamp {text}")))
}

fn failed(operation: &'static str) -> impl Fn(rusqlite::Error) -> TaskError {
    move |source| TaskError::Operation {
        operation,
        source: Box::new(source),
    }
}
