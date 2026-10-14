use std::str::FromStr as _;

use chrono::DateTime;
use chrono::Utc;
use kernel::queue::JobName;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;
use wire::JsonObject;

use crate::SqliteStore;
use crate::error::SqliteError;
use crate::time;

pub(crate) const EMBED_TASK: &str = "embed-target";
const EMBED_MAX_ATTEMPTS: u32 = 5;
const ERROR_CHARS: usize = 2_000;
const UNIQUE_JOB_KEY: i32 = 2067;
const DELAYS_MS: [u64; 9] = [
    30_000, 30_000, 60_000, 120_000, 240_000, 480_000, 960_000, 1_920_000, 3_600_000,
];
const DELAY_CAP_MS: u64 = 3_600_000;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Dead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JobPayload {
    Object(JsonObject),
    Text(String),
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimedSqliteJob {
    pub id: String,
    pub task: String,
    pub key: Option<String>,
    pub payload: JobPayload,
    pub attempts: u32,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailOutcome {
    Retry,
    Dead,
}

impl std::fmt::Display for FailOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Retry => formatter.write_str("retry"),
            Self::Dead => formatter.write_str("dead"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobCount {
    pub task: String,
    pub state: JobState,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobInsert {
    pub task: String,
    pub key: Option<String>,
    pub payload: String,
    pub run_at: DateTime<Utc>,
    pub max_attempts: u32,
    pub now: DateTime<Utc>,
}

impl JobInsert {
    #[must_use]
    pub fn new(task: impl Into<String>, payload: String, now: DateTime<Utc>) -> Self {
        Self {
            task: task.into(),
            key: None,
            payload,
            run_at: now,
            max_attempts: EMBED_MAX_ATTEMPTS,
            now,
        }
    }

    #[must_use]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    #[must_use]
    pub fn max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    #[must_use]
    pub fn run_at(mut self, run_at: DateTime<Utc>) -> Self {
        self.run_at = run_at;
        self
    }
}

#[derive(Debug, Serialize)]
struct EmbedJob<'a> {
    #[serde(rename = "targetTable")]
    target_table: &'a str,
    #[serde(rename = "targetId")]
    target_id: &'a str,
}

#[derive(Clone, Copy)]
enum ClaimKind {
    Inherent,
    Port,
}

struct DueRow {
    id: String,
    task: String,
    key: Option<String>,
    payload: String,
    max_attempts: i64,
}

#[must_use]
pub fn retry_delay_ms(attempt: u32) -> u64 {
    DELAYS_MS
        .iter()
        .zip(0_u32..)
        .find_map(|(delay, index)| (index == attempt).then_some(*delay))
        .unwrap_or(DELAY_CAP_MS)
}

impl SqliteStore {
    pub async fn enqueue_job(&self, job: JobInsert) -> Result<bool, SqliteError> {
        self.with_conn(move |conn| insert_job(conn, &job)).await
    }

    pub async fn claim_tasks(
        &self,
        tasks: Vec<String>,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimedSqliteJob>, SqliteError> {
        self.with_conn(move |conn| claim_one(conn, &tasks, now, ClaimKind::Inherent))
            .await
    }

    pub(crate) async fn claim_port(
        &self,
        tasks: Vec<String>,
        now: DateTime<Utc>,
    ) -> Result<Option<ClaimedSqliteJob>, SqliteError> {
        self.with_conn(move |conn| claim_one(conn, &tasks, now, ClaimKind::Port))
            .await
    }

    pub async fn fail_job(
        &self,
        id: String,
        error: String,
        now: DateTime<Utc>,
    ) -> Result<FailOutcome, SqliteError> {
        self.with_conn(move |conn| fail_one(conn, &id, &error, now))
            .await
    }

    pub async fn complete_job(&self, id: String) -> Result<(), SqliteError> {
        self.with_conn(move |conn| delete_job(conn, &id)).await
    }

    pub async fn revive(
        &self,
        key: String,
        payload: String,
        now: DateTime<Utc>,
    ) -> Result<bool, SqliteError> {
        self.with_conn(move |conn| revive_one(conn, &key, &payload, now))
            .await
    }

    pub async fn requeue_dead(
        &self,
        task: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<u64, SqliteError> {
        self.with_conn(move |conn| requeue(conn, task.as_deref(), now))
            .await
    }

    pub async fn recover_running(&self, now: DateTime<Utc>) -> Result<u64, SqliteError> {
        self.with_conn(move |conn| recover(conn, now)).await
    }

    pub async fn job_counts(&self) -> Result<Vec<JobCount>, SqliteError> {
        self.with_conn(counts).await
    }

    pub(crate) async fn replace_job(
        &self,
        key: String,
        payload: String,
        run_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<(), SqliteError> {
        self.with_conn(move |conn| replace(conn, &key, &payload, run_at, now))
            .await
    }
}

pub(crate) fn enqueue_embed(
    conn: &Connection,
    table: &str,
    id: &str,
    now: DateTime<Utc>,
) -> Result<bool, SqliteError> {
    enqueue_embed_due(conn, table, id, now, now)
}

pub(crate) fn enqueue_embed_due(
    conn: &Connection,
    table: &str,
    id: &str,
    now: DateTime<Utc>,
    run_at: DateTime<Utc>,
) -> Result<bool, SqliteError> {
    let payload = serde_json::to_string(&EmbedJob {
        target_table: table,
        target_id: id,
    })
    .map_err(|source| SqliteError::Encode {
        column: "payload",
        source,
    })?;
    let job = JobInsert::new(EMBED_TASK, payload, now)
        .key(format!("embed-{table}-{id}"))
        .run_at(run_at);
    insert_job(conn, &job)
}

fn insert_job(conn: &Connection, job: &JobInsert) -> Result<bool, SqliteError> {
    let id = Uuid::new_v4().to_string();
    let stamped = time::stamp(job.now);
    let run_at = time::stamp(job.run_at);
    let max_attempts = i64::from(job.max_attempts);
    match conn.execute(
        "INSERT INTO ros_jobs (
            id, task, job_key, payload, run_at, attempts, max_attempts, state, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, 0, ?6, 'queued', ?7, ?7)",
        params![id, job.task, job.key, job.payload, run_at, max_attempts, stamped],
    ) {
        Ok(_) => Ok(true),
        Err(error) if duplicate_key(&error) => Ok(false),
        Err(error) => Err(SqliteError::query("job enqueue")(error)),
    }
}

fn claim_one(
    conn: &mut Connection,
    tasks: &[String],
    now: DateTime<Utc>,
    kind: ClaimKind,
) -> Result<Option<ClaimedSqliteJob>, SqliteError> {
    if tasks.is_empty() {
        return Ok(None);
    }
    let tx = conn
        .transaction()
        .map_err(SqliteError::query("job claim begin"))?;
    let Some(row) = select_due(&tx, tasks, now)? else {
        return Ok(None);
    };
    let payload = match accepted_payload(&row, kind) {
        Ok(payload) => payload,
        Err(error) => {
            reject_claim(&tx, &row.id, &error, now)?;
            tx.commit()
                .map_err(SqliteError::query("job claim commit"))?;
            return Err(error);
        }
    };
    let attempts = mark_running(&tx, &row.id, now)?;
    let claimed = ClaimedSqliteJob {
        id: row.id,
        task: row.task,
        key: row.key,
        payload,
        attempts,
        max_attempts: u32::try_from(row.max_attempts).map_err(SqliteError::Count)?,
    };
    tx.commit()
        .map_err(SqliteError::query("job claim commit"))?;
    Ok(Some(claimed))
}

fn select_due(
    conn: &Connection,
    tasks: &[String],
    now: DateTime<Utc>,
) -> Result<Option<DueRow>, SqliteError> {
    let sql = due_sql(tasks.len());
    let stamped = time::stamp(now);
    let mut bound: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(tasks.len().saturating_add(1));
    bound.push(&stamped);
    for task in tasks {
        bound.push(task);
    }
    conn.query_row(&sql, bound.as_slice(), read_due)
        .optional()
        .map_err(SqliteError::query("job claim"))
}

fn due_sql(count: usize) -> String {
    let mut slots = String::new();
    for index in 0..count {
        if index > 0 {
            slots.push_str(", ");
        }
        slots.push('?');
        slots.push_str(&(index.saturating_add(2)).to_string());
    }
    format!(
        "SELECT id, task, job_key, payload, max_attempts
           FROM ros_jobs
          WHERE state = 'queued' AND run_at <= ?1 AND task IN ({slots})
          ORDER BY run_at, rowid
          LIMIT 1"
    )
}

fn read_due(row: &rusqlite::Row<'_>) -> rusqlite::Result<DueRow> {
    Ok(DueRow {
        id: row.get(0)?,
        task: row.get(1)?,
        key: row.get(2)?,
        payload: row.get(3)?,
        max_attempts: row.get(4)?,
    })
}

fn accepted_payload(row: &DueRow, kind: ClaimKind) -> Result<JobPayload, SqliteError> {
    let payload = serde_json::from_str(&row.payload)
        .map_err(|source| SqliteError::StoredPayload { source })?;
    match kind {
        ClaimKind::Inherent => Ok(payload),
        ClaimKind::Port => port_payload(&row.task, payload),
    }
}

fn port_payload(task: &str, payload: JobPayload) -> Result<JobPayload, SqliteError> {
    JobName::from_str(task).map_err(|error| SqliteError::ClaimRejected {
        message: error.to_string(),
    })?;
    if let JobPayload::Object(_) = &payload {
        Ok(payload)
    } else {
        Err(SqliteError::PayloadNotObject)
    }
}

fn mark_running(conn: &Connection, id: &str, now: DateTime<Utc>) -> Result<u32, SqliteError> {
    let stamped = time::stamp(now);
    let attempts = conn
        .query_row(
            "UPDATE ros_jobs
                SET state = 'running', attempts = attempts + 1, locked_at = ?2, updated_at = ?2
              WHERE id = ?1 AND state = 'queued'
              RETURNING attempts",
            params![id, stamped],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(SqliteError::query("job claim"))?;
    let Some(attempts) = attempts else {
        return Err(SqliteError::MissingJob { id: id.to_owned() });
    };
    u32::try_from(attempts).map_err(SqliteError::Count)
}

fn reject_claim(
    conn: &Connection,
    id: &str,
    error: &SqliteError,
    now: DateTime<Utc>,
) -> Result<(), SqliteError> {
    let clipped: String = error.to_string().chars().take(ERROR_CHARS).collect();
    conn.execute(
        "UPDATE ros_jobs
            SET state = 'dead', last_error = ?2, locked_at = NULL, updated_at = ?3
          WHERE id = ?1 AND state = 'queued'",
        params![id, clipped, time::stamp(now)],
    )
    .map_err(SqliteError::query("job claim reject"))?;
    Ok(())
}

fn fail_one(
    conn: &mut Connection,
    id: &str,
    error: &str,
    now: DateTime<Utc>,
) -> Result<FailOutcome, SqliteError> {
    let tx = conn
        .transaction()
        .map_err(SqliteError::query("job fail begin"))?;
    let found = tx
        .query_row(
            "SELECT attempts, max_attempts FROM ros_jobs WHERE id = ?1",
            [id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(SqliteError::query("job fail load"))?;
    let Some((attempts, max_attempts)) = found else {
        return Err(SqliteError::MissingJob { id: id.to_owned() });
    };
    let attempts = u32::try_from(attempts).map_err(SqliteError::Count)?;
    let max_attempts = u32::try_from(max_attempts).map_err(SqliteError::Count)?;
    let outcome = if attempts >= max_attempts {
        FailOutcome::Dead
    } else {
        FailOutcome::Retry
    };
    let state = match outcome {
        FailOutcome::Dead => JobState::Dead,
        FailOutcome::Retry => JobState::Queued,
    };
    let run_at = later(now, attempts)?;
    let clipped: String = error.chars().take(ERROR_CHARS).collect();
    tx.execute(
        "UPDATE ros_jobs
            SET state = ?2, run_at = ?3, last_error = ?4, locked_at = NULL, updated_at = ?5
          WHERE id = ?1",
        params![
            id,
            state.to_string(),
            time::stamp(run_at),
            clipped,
            time::stamp(now)
        ],
    )
    .map_err(SqliteError::query("job fail"))?;
    tx.commit().map_err(SqliteError::query("job fail commit"))?;
    Ok(outcome)
}

fn later(now: DateTime<Utc>, attempt: u32) -> Result<DateTime<Utc>, SqliteError> {
    let delay = i64::try_from(retry_delay_ms(attempt)).map_err(|_| SqliteError::Clock)?;
    now.checked_add_signed(chrono::Duration::milliseconds(delay))
        .ok_or(SqliteError::Clock)
}

fn delete_job(conn: &Connection, id: &str) -> Result<(), SqliteError> {
    let deleted = conn
        .execute("DELETE FROM ros_jobs WHERE id = ?1", [id])
        .map_err(SqliteError::query("job complete"))?;
    if deleted == 0 {
        Err(SqliteError::MissingJob { id: id.to_owned() })
    } else {
        Ok(())
    }
}

fn revive_one(
    conn: &Connection,
    key: &str,
    payload: &str,
    now: DateTime<Utc>,
) -> Result<bool, SqliteError> {
    let stamped = time::stamp(now);
    let changed = conn
        .execute(
            "UPDATE ros_jobs
                SET payload = ?2, run_at = ?3, attempts = 0, state = 'queued',
                    locked_at = NULL, updated_at = ?3
              WHERE job_key = ?1 AND state = 'dead'",
            params![key, payload, stamped],
        )
        .map_err(SqliteError::query("job revive"))?;
    Ok(changed > 0)
}

fn requeue(conn: &Connection, task: Option<&str>, now: DateTime<Utc>) -> Result<u64, SqliteError> {
    let stamped = time::stamp(now);
    let changed = conn
        .execute(
            "UPDATE ros_jobs
                SET state = 'queued', attempts = 0, run_at = ?1, locked_at = NULL, updated_at = ?1
              WHERE state = 'dead' AND (?2 IS NULL OR task = ?2)",
            params![stamped, task],
        )
        .map_err(SqliteError::query("job requeue"))?;
    u64::try_from(changed).map_err(SqliteError::Count)
}

fn recover(conn: &Connection, now: DateTime<Utc>) -> Result<u64, SqliteError> {
    let changed = conn
        .execute(
            "UPDATE ros_jobs
                SET state = 'queued', locked_at = NULL, updated_at = ?1
              WHERE state = 'running'",
            [time::stamp(now)],
        )
        .map_err(SqliteError::query("job recover"))?;
    u64::try_from(changed).map_err(SqliteError::Count)
}

fn counts(conn: &mut Connection) -> Result<Vec<JobCount>, SqliteError> {
    let mut statement = conn
        .prepare(
            "SELECT task, state, count(*) FROM ros_jobs
              GROUP BY task, state ORDER BY task, state",
        )
        .map_err(SqliteError::query("job counts"))?;
    let mut rows = statement
        .query([])
        .map_err(SqliteError::query("job counts"))?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(SqliteError::query("job counts"))? {
        let task: String = row.get(0).map_err(SqliteError::query("job counts"))?;
        let state: String = row.get(1).map_err(SqliteError::query("job counts"))?;
        let count: i64 = row.get(2).map_err(SqliteError::query("job counts"))?;
        out.push(count_row(task, state, count)?);
    }
    Ok(out)
}

fn count_row(task: String, state: String, count: i64) -> Result<JobCount, SqliteError> {
    let state = state
        .parse()
        .map_err(|_| SqliteError::StoredState { value: state })?;
    Ok(JobCount {
        task,
        state,
        count: u64::try_from(count).map_err(SqliteError::Count)?,
    })
}

fn replace(
    conn: &Connection,
    key: &str,
    payload: &str,
    run_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(), SqliteError> {
    let changed = conn
        .execute(
            "UPDATE ros_jobs
                SET payload = ?2,
                    run_at = ?3,
                    updated_at = ?4,
                    attempts = CASE WHEN state = 'dead' THEN 0 ELSE attempts END,
                    state = CASE WHEN state = 'dead' THEN 'queued' ELSE state END,
                    locked_at = CASE WHEN state = 'dead' THEN NULL ELSE locked_at END
              WHERE job_key = ?1",
            params![key, payload, time::stamp(run_at), time::stamp(now)],
        )
        .map_err(SqliteError::query("job replace"))?;
    if changed == 0 {
        Err(SqliteError::MissingJob { id: key.to_owned() })
    } else {
        Ok(())
    }
}

fn duplicate_key(error: &rusqlite::Error) -> bool {
    if let rusqlite::Error::SqliteFailure(sqlite, Some(message)) = error {
        sqlite.extended_code == UNIQUE_JOB_KEY && message.contains("ros_jobs.job_key")
    } else {
        false
    }
}
