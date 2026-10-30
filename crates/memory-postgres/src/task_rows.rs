use std::str::FromStr;
use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use sqlx::PgConnection;
use sqlx::Row;
use sqlx::postgres::PgRow;
use sqlx::types::Json;
use uuid::Uuid;

use kernel::tasks::Budget;
use kernel::tasks::Criterion;
use kernel::tasks::Finish;
use kernel::tasks::Harness;
use kernel::tasks::Outcome;
use kernel::tasks::Parent;
use kernel::tasks::ParentSource;
use kernel::tasks::Target;
use kernel::tasks::Task;
use kernel::tasks::TaskError;
use kernel::tasks::TaskFilter;
use kernel::tasks::TaskStatus;
use kernel::tasks::Usage;

use crate::task_jobs::map_sql;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct StoredBudget {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_turns: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_wall_clock_ms: Option<u64>,
}

pub(crate) async fn insert(conn: &mut PgConnection, task: &Task) -> Result<(), TaskError> {
    sqlx::query(
        "INSERT INTO rh_tasks (id, goal, acceptance_criteria, budget, status, agent, preset, node, \
         harness, model, effort, cwd, permission_mode, parent_session_id, parent_task_id, \
         parent_source, depth, requested_by, owner_user_id, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, \
         $18, $19, $20)",
    )
    .bind(task.id)
    .bind(&task.goal)
    .bind(Json(&task.criteria))
    .bind(Json(budget_out(task.budget)?))
    .bind(task.status.to_string())
    .bind(&task.target.agent)
    .bind(&task.target.preset)
    .bind(&task.target.node)
    .bind(task.target.harness.map(|harness| harness.to_string()))
    .bind(&task.target.model)
    .bind(&task.target.effort)
    .bind(&task.target.cwd)
    .bind(&task.target.permission_mode)
    .bind(&task.parent.session)
    .bind(task.parent.task)
    .bind(task.parent.source.map(|source| source.to_string()))
    .bind(i16::from(task.parent.depth))
    .bind(&task.requested_by)
    .bind(&task.owner)
    .bind(task.created_at)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("insert task", source))?;
    Ok(())
}

pub(crate) async fn load(conn: &mut PgConnection, id: Uuid) -> Result<Task, TaskError> {
    let row = sqlx::query(
        "SELECT id, goal, acceptance_criteria, budget, status, result, error, usage, agent, preset, node, harness, model, effort, cwd, permission_mode, parent_session_id, parent_task_id, parent_source, depth, child_session_id, requested_by, owner_user_id, created_at, started_at, completed_at, alive_at FROM rh_tasks WHERE id = $1",
    )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|source| map_sql("load task", source))?;
    match row {
        Some(row) => decode(&row),
        None => Err(TaskError::Missing { id }),
    }
}

pub(crate) async fn by_child(
    conn: &mut PgConnection,
    session: &str,
) -> Result<Option<Task>, TaskError> {
    sqlx::query(
        "SELECT id, goal, acceptance_criteria, budget, status, result, error, usage, agent, preset, node, harness, model, effort, cwd, permission_mode, parent_session_id, parent_task_id, parent_source, depth, child_session_id, requested_by, owner_user_id, created_at, started_at, completed_at, alive_at FROM rh_tasks WHERE child_session_id = $1",
    )
        .bind(session)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|source| map_sql("task by child", source))?
        .as_ref()
        .map(decode)
        .transpose()
}

pub(crate) async fn list(
    conn: &mut PgConnection,
    filter: &TaskFilter,
) -> Result<Vec<Task>, TaskError> {
    let rows = sqlx::query(
        "SELECT id, goal, acceptance_criteria, budget, status, result, error, usage, agent, preset, node, harness, model, effort, cwd, permission_mode, parent_session_id, parent_task_id, parent_source, depth, child_session_id, requested_by, owner_user_id, created_at, started_at, completed_at, alive_at FROM rh_tasks WHERE ($1::text IS NULL OR status = $1) \
         AND ($2::text IS NULL OR agent = $2) ORDER BY created_at DESC, id DESC LIMIT $3",
    )
    .bind(filter.status.map(|status| status.to_string()))
    .bind(filter.agent.as_deref())
    .bind(filter.limit.map(i64::from))
    .fetch_all(&mut *conn)
    .await
    .map_err(|source| map_sql("list tasks", source))?;
    rows.iter().map(decode).collect()
}

pub(crate) async fn begin(
    conn: &mut PgConnection,
    id: Uuid,
    node: &str,
    now: DateTime<Utc>,
) -> Result<bool, TaskError> {
    let done = sqlx::query(
        "UPDATE rh_tasks SET status = 'running', started_at = $3, alive_at = $3 \
         WHERE id = $1 AND node = $2 AND status = 'queued'",
    )
    .bind(id)
    .bind(node)
    .bind(now)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("begin task", source))?;
    Ok(done.rows_affected() == 1)
}

pub(crate) async fn attach(
    conn: &mut PgConnection,
    id: Uuid,
    child: &str,
    harness: Harness,
) -> Result<(), TaskError> {
    sqlx::query("UPDATE rh_tasks SET child_session_id = $2, harness = $3 WHERE id = $1")
        .bind(id)
        .bind(child)
        .bind(harness.to_string())
        .execute(&mut *conn)
        .await
        .map_err(|source| map_sql("attach child", source))?;
    Ok(())
}

pub(crate) async fn flip(
    conn: &mut PgConnection,
    id: Uuid,
    status: TaskStatus,
) -> Result<bool, TaskError> {
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
    let done = sqlx::query("UPDATE rh_tasks SET status = $3 WHERE id = $1 AND status = $2")
        .bind(id)
        .bind(from.to_string())
        .bind(status.to_string())
        .execute(&mut *conn)
        .await
        .map_err(|source| map_sql("flip task", source))?;
    Ok(done.rows_affected() == 1)
}

pub(crate) async fn alive(
    conn: &mut PgConnection,
    ids: &[Uuid],
    now: DateTime<Utc>,
) -> Result<(), TaskError> {
    sqlx::query(
        "UPDATE rh_tasks SET alive_at = $2 \
         WHERE id = ANY($1) AND status IN ('running', 'awaiting-input')",
    )
    .bind(ids)
    .bind(now)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("task liveness", source))?;
    Ok(())
}

pub(crate) async fn finish(
    conn: &mut PgConnection,
    id: Uuid,
    finish: &Finish,
) -> Result<bool, TaskError> {
    let from: &[&str] = if finish.status == TaskStatus::Completed {
        &["running", "awaiting-input"]
    } else {
        &["queued", "running", "awaiting-input"]
    };
    let done = sqlx::query(
        "UPDATE rh_tasks SET status = $2, result = $3, error = $4, usage = $5, completed_at = $6 \
         WHERE id = $1 AND status = ANY($7)",
    )
    .bind(id)
    .bind(finish.status.to_string())
    .bind(finish.result.as_ref().map(Json))
    .bind(&finish.error)
    .bind(finish.usage.as_ref().map(Json))
    .bind(finish.at)
    .bind(from)
    .execute(&mut *conn)
    .await
    .map_err(|source| map_sql("finish task", source))?;
    Ok(done.rows_affected() == 1)
}

pub(crate) async fn kill(
    conn: &mut PgConnection,
    id: Uuid,
    now: DateTime<Utc>,
) -> Result<TaskStatus, TaskError> {
    let prior = sqlx::query("SELECT status FROM rh_tasks WHERE id = $1 FOR UPDATE")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|source| map_sql("kill task", source))?
        .ok_or(TaskError::Missing { id })?;
    let prior = status(&prior.try_get::<String, _>("status").map_err(stored)?)?;
    if prior.terminal() {
        return Ok(prior);
    }
    sqlx::query("UPDATE rh_tasks SET status = 'killed', completed_at = $2 WHERE id = $1")
        .bind(id)
        .bind(now)
        .execute(&mut *conn)
        .await
        .map_err(|source| map_sql("kill task", source))?;
    Ok(prior)
}

pub(crate) async fn orphans(
    conn: &mut PgConnection,
    node: &str,
    now: DateTime<Utc>,
) -> Result<Vec<Uuid>, TaskError> {
    sqlx::query_scalar(
        "UPDATE rh_tasks SET status = 'failed', completed_at = $2, \
         error = 'its gateway restarted' \
         WHERE node = $1 AND status IN ('running', 'awaiting-input') RETURNING id",
    )
    .bind(node)
    .bind(now)
    .fetch_all(&mut *conn)
    .await
    .map_err(|source| map_sql("fail orphaned tasks", source))
}

pub(crate) async fn reap(
    conn: &mut PgConnection,
    stale_before: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<Vec<Uuid>, TaskError> {
    sqlx::query_scalar(
        "UPDATE rh_tasks SET status = 'failed', completed_at = $2, \
         error = 'its node stopped running it' \
         WHERE status IN ('running', 'awaiting-input') \
         AND (alive_at IS NULL OR alive_at < $1) RETURNING id",
    )
    .bind(stale_before)
    .bind(now)
    .fetch_all(&mut *conn)
    .await
    .map_err(|source| map_sql("reap tasks", source))
}

fn decode(row: &PgRow) -> Result<Task, TaskError> {
    let Json(criteria): Json<Vec<Criterion>> = get(row, "acceptance_criteria")?;
    let Json(budget): Json<StoredBudget> = get(row, "budget")?;
    let result: Option<Json<Outcome>> = get(row, "result")?;
    let usage: Option<Json<Usage>> = get(row, "usage")?;
    let source: Option<String> = get(row, "parent_source")?;
    let depth: i16 = get(row, "depth")?;
    Ok(Task {
        id: get(row, "id")?,
        goal: get(row, "goal")?,
        criteria,
        budget: Budget {
            wall_clock: budget.max_wall_clock_ms.map(Duration::from_millis),
            max_turns: budget.max_turns,
        },
        status: status(&get::<String>(row, "status")?)?,
        target: Target {
            agent: get(row, "agent")?,
            preset: get(row, "preset")?,
            node: get(row, "node")?,
            harness: get::<Option<String>>(row, "harness")?
                .as_deref()
                .map(parse::<Harness>)
                .transpose()?,
            model: get(row, "model")?,
            effort: get(row, "effort")?,
            cwd: get(row, "cwd")?,
            permission_mode: get(row, "permission_mode")?,
        },
        parent: Parent {
            session: get(row, "parent_session_id")?,
            task: get(row, "parent_task_id")?,
            depth: u8::try_from(depth).map_err(|_| TaskError::Stored("depth".to_owned()))?,
            source: source
                .map(|text| ParentSource::from_str(&text))
                .transpose()
                .map_err(|_| TaskError::Stored("parent_source".to_owned()))?,
        },
        child: get(row, "child_session_id")?,
        result: result.map(|Json(result)| result),
        error: get(row, "error")?,
        usage: usage.map(|Json(usage)| usage),
        requested_by: get(row, "requested_by")?,
        owner: get(row, "owner_user_id")?,
        created_at: get(row, "created_at")?,
        started_at: get(row, "started_at")?,
        completed_at: get(row, "completed_at")?,
        alive_at: get(row, "alive_at")?,
    })
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

fn get<'r, T>(row: &'r PgRow, column: &str) -> Result<T, TaskError>
where
    T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(column).map_err(stored)
}

fn parse<T: FromStr>(text: &str) -> Result<T, TaskError> {
    T::from_str(text).map_err(|_| TaskError::Stored(format!("task field {text}")))
}

fn status(text: &str) -> Result<TaskStatus, TaskError> {
    TaskStatus::from_str(text).map_err(|_| TaskError::Stored(format!("task status {text}")))
}

fn stored(source: sqlx::Error) -> TaskError {
    TaskError::Operation {
        operation: "read task row",
        source: Box::new(source),
    }
}
