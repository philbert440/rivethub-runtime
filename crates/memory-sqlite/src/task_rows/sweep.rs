use chrono::DateTime;
use chrono::Utc;
use kernel::tasks::TaskError;
use rusqlite::Connection;
use rusqlite::params;
use wire::task::TaskBudget;

use super::decode;
use super::parse_time;
use super::sql_error;
use super::stamp;
use super::u32_in;

const AWAITING_DEFAULT_MS: u64 = 86_400_000;

pub(super) fn running(
    conn: &Connection,
    worker: &str,
    now: DateTime<Utc>,
) -> Result<(), TaskError> {
    for row in running_rows(conn, worker)? {
        if running_stale(&row, now)? == Some(true) {
            settle_running(conn, &row, now)?;
        }
    }
    Ok(())
}

pub(super) fn awaiting(conn: &Connection, now: DateTime<Utc>) -> Result<(), TaskError> {
    for row in awaiting_rows(conn)? {
        if awaiting_expired(&row, now)? {
            conn.execute(
                "UPDATE rh_tasks SET status = 'timeout', error = 'awaiting-input expired',
                    completed_at = ?2 WHERE id = ?1",
                params![row.id, stamp(now)],
            )
            .map_err(|source| sql_error("sweep awaiting", source))?;
        }
    }
    Ok(())
}

fn settle_running(
    conn: &Connection,
    row: &RunningRow,
    now: DateTime<Utc>,
) -> Result<(), TaskError> {
    let attempt = u32_in(row.attempt)?;
    let max_attempts = u32_in(row.max_attempts)?;
    if attempt < max_attempts {
        conn.execute(
            "UPDATE rh_tasks SET status = 'queued', claimed_by = NULL, completed_at = NULL WHERE id = ?1",
            params![row.id],
        )
        .map_err(|source| sql_error("sweep running", source))?;
    } else {
        conn.execute(
            "UPDATE rh_tasks SET status = 'failed', error = 'worker_restarted', completed_at = ?2 WHERE id = ?1",
            params![row.id, stamp(now)],
        )
        .map_err(|source| sql_error("sweep running", source))?;
    }
    Ok(())
}

struct RunningRow {
    id: String,
    attempt: i64,
    max_attempts: i64,
    heartbeat: Option<String>,
    started: Option<String>,
}

fn running_rows(conn: &Connection, worker: &str) -> Result<Vec<RunningRow>, TaskError> {
    let mut statement = conn
        .prepare(
            "SELECT id, attempt, max_attempts, last_heartbeat_at, started_at
               FROM rh_tasks WHERE status = 'running' AND claimed_by = ?1",
        )
        .map_err(|source| sql_error("sweep running", source))?;
    let rows = statement
        .query_map(params![worker], |row| {
            Ok(RunningRow {
                id: row.get(0)?,
                attempt: row.get(1)?,
                max_attempts: row.get(2)?,
                heartbeat: row.get(3)?,
                started: row.get(4)?,
            })
        })
        .map_err(|source| sql_error("sweep running", source))?;
    collect(rows, "sweep running")
}

fn running_stale(row: &RunningRow, now: DateTime<Utc>) -> Result<Option<bool>, TaskError> {
    let text = match &row.heartbeat {
        Some(text) => text.as_str(),
        None => match &row.started {
            Some(text) => text.as_str(),
            None => return Ok(None),
        },
    };
    let anchor = parse_time(text)?;
    let cutoff = now
        .checked_sub_signed(chrono::Duration::seconds(90))
        .ok_or_else(|| TaskError::Stored("running cutoff overflow".to_owned()))?;
    Ok(Some(anchor < cutoff))
}

struct AwaitingRow {
    id: String,
    heartbeat: Option<String>,
    started: Option<String>,
    created: String,
    budget: String,
}

fn awaiting_rows(conn: &Connection) -> Result<Vec<AwaitingRow>, TaskError> {
    let mut statement = conn
        .prepare(
            "SELECT id, last_heartbeat_at, started_at, created_at, budget
               FROM rh_tasks WHERE status = 'awaiting-input'",
        )
        .map_err(|source| sql_error("sweep awaiting", source))?;
    let rows = statement
        .query_map([], |row| {
            Ok(AwaitingRow {
                id: row.get(0)?,
                heartbeat: row.get(1)?,
                started: row.get(2)?,
                created: row.get(3)?,
                budget: row.get(4)?,
            })
        })
        .map_err(|source| sql_error("sweep awaiting", source))?;
    collect(rows, "sweep awaiting")
}

fn awaiting_expired(row: &AwaitingRow, now: DateTime<Utc>) -> Result<bool, TaskError> {
    let text = if let Some(text) = row.heartbeat.as_deref() {
        text
    } else if let Some(text) = row.started.as_deref() {
        text
    } else {
        row.created.as_str()
    };
    let anchor = parse_time(text)?;
    let budget: TaskBudget = decode(&row.budget)?;
    let millis = match budget.max_wall_clock_ms {
        Some(value) => value,
        None => AWAITING_DEFAULT_MS,
    };
    let span = chrono::Duration::from_std(std::time::Duration::from_millis(millis))
        .map_err(|_| TaskError::Stored("awaiting budget overflow".to_owned()))?;
    let deadline = anchor
        .checked_add_signed(span)
        .ok_or_else(|| TaskError::Stored("awaiting deadline overflow".to_owned()))?;
    Ok(now > deadline)
}

fn collect<T>(
    rows: impl IntoIterator<Item = rusqlite::Result<T>>,
    operation: &'static str,
) -> Result<Vec<T>, TaskError> {
    let mut collected = Vec::new();
    for row in rows {
        collected.push(row.map_err(|source| sql_error(operation, source))?);
    }
    Ok(collected)
}
