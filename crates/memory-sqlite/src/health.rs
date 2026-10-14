use std::collections::BTreeMap;

use chrono::DateTime;
use chrono::Utc;
use rusqlite::Connection;
use serde::Serialize;
use wire::gateway::memory::EmbedStatus;
use wire::gateway::memory::MemoryHealthStatus;
use wire::gateway::memory::QueueStatus;

use crate::SqliteStore;
use crate::error::SqliteError;
use crate::queue::JobState;
use crate::time;

const EMBED_MISSING: &str = "embedding endpoint not configured";
const EMBED_IMPACT: &str = "Keyword matching still works; meaning-based ranking is offline.";
const CAPTURE_IMPACT: &str = "Capture progress is not measured by this endpoint yet.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureProgress {
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteEmbedHealth {
    pub status: EmbedStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub impact: Option<String>,
    pub checked_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteQueueHealth {
    pub task: String,
    pub pending: u64,
    pub running: u64,
    pub scheduled: u64,
    pub dead: u64,
    pub oldest_pending_minutes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteCompaction {
    pub eligible: u64,
    pub active_tail: u64,
    pub below_floor: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteCaptureHealth {
    pub status: CaptureProgress,
    pub impact: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteHealth {
    pub status: MemoryHealthStatus,
    pub observed_at: String,
    pub embeddings: SqliteEmbedHealth,
    pub embed_queue_depth: u64,
    pub failed_embeddings: u64,
    pub skipped_embeddings: u64,
    pub queue_status: QueueStatus,
    pub queues: Vec<SqliteQueueHealth>,
    pub compaction: SqliteCompaction,
    pub capture: SqliteCaptureHealth,
}

struct Bucket {
    pending: u64,
    running: u64,
    scheduled: u64,
    dead: u64,
    oldest: Option<DateTime<Utc>>,
}

impl Bucket {
    fn new() -> Self {
        Self {
            pending: 0,
            running: 0,
            scheduled: 0,
            dead: 0,
            oldest: None,
        }
    }
}

impl SqliteStore {
    pub async fn health(&self, now: DateTime<Utc>) -> Result<SqliteHealth, SqliteError> {
        let armed = self.embed_armed();
        self.with_conn(move |conn| read_health(conn, now, armed))
            .await
    }
}

fn read_health(
    conn: &Connection,
    now: DateTime<Utc>,
    armed: bool,
) -> Result<SqliteHealth, SqliteError> {
    let failed = scalar(
        conn,
        "SELECT
            (SELECT count(*) FROM ros_messages WHERE embed_failures > 0)
          + (SELECT count(*) FROM ros_summaries WHERE embed_failures > 0)",
        "failed embeddings",
    )?;
    let skipped = scalar(
        conn,
        "SELECT
            (SELECT count(*) FROM ros_messages WHERE embed_status = 'unembeddable')
          + (SELECT count(*) FROM ros_summaries WHERE embed_status = 'unembeddable')",
        "skipped embeddings",
    )?;
    let depth = scalar(
        conn,
        "SELECT count(*) FROM ros_jobs WHERE task = 'embed-target' AND state <> 'dead'",
        "embed queue depth",
    )?;
    let messages = scalar(conn, "SELECT count(*) FROM ros_messages", "message count")?;
    let embeddings = embed_health(armed, now);
    let status = if embeddings.status == EmbedStatus::Unavailable || failed > 0 {
        MemoryHealthStatus::Degraded
    } else {
        MemoryHealthStatus::Ok
    };
    Ok(SqliteHealth {
        status,
        observed_at: time::stamp(now),
        embeddings,
        embed_queue_depth: depth,
        failed_embeddings: failed,
        skipped_embeddings: skipped,
        queue_status: QueueStatus::Unavailable,
        queues: queue_health(conn, now)?,
        compaction: SqliteCompaction {
            eligible: 0,
            active_tail: 0,
            below_floor: messages,
        },
        capture: SqliteCaptureHealth {
            status: CaptureProgress::Unknown,
            impact: CAPTURE_IMPACT,
        },
    })
}

fn embed_health(armed: bool, now: DateTime<Utc>) -> SqliteEmbedHealth {
    let checked_at = time::stamp(now);
    if armed {
        SqliteEmbedHealth {
            status: EmbedStatus::Ok,
            error: None,
            impact: None,
            checked_at,
        }
    } else {
        SqliteEmbedHealth {
            status: EmbedStatus::Unavailable,
            error: Some(EMBED_MISSING.to_owned()),
            impact: Some(EMBED_IMPACT.to_owned()),
            checked_at,
        }
    }
}

fn scalar(conn: &Connection, sql: &str, operation: &'static str) -> Result<u64, SqliteError> {
    let value: i64 = conn
        .query_row(sql, [], |row| row.get(0))
        .map_err(SqliteError::query(operation))?;
    u64::try_from(value).map_err(SqliteError::Count)
}

fn queue_health(
    conn: &Connection,
    now: DateTime<Utc>,
) -> Result<Vec<SqliteQueueHealth>, SqliteError> {
    let mut statement = conn
        .prepare("SELECT task, state, run_at, created_at FROM ros_jobs ORDER BY task, rowid")
        .map_err(SqliteError::query("job health"))?;
    let mut rows = statement
        .query([])
        .map_err(SqliteError::query("job health"))?;
    let mut groups: BTreeMap<String, Bucket> = BTreeMap::new();
    while let Some(row) = rows.next().map_err(SqliteError::query("job health"))? {
        let task: String = row.get(0).map_err(SqliteError::query("job health"))?;
        let state: String = row.get(1).map_err(SqliteError::query("job health"))?;
        let run_at: String = row.get(2).map_err(SqliteError::query("job health"))?;
        let created_at: String = row.get(3).map_err(SqliteError::query("job health"))?;
        let bucket = groups.entry(task).or_insert_with(Bucket::new);
        add_job(bucket, &state, &run_at, &created_at, now)?;
    }
    let mut queues = Vec::new();
    for (task, bucket) in groups {
        queues.push(SqliteQueueHealth {
            task,
            pending: bucket.pending,
            running: bucket.running,
            scheduled: bucket.scheduled,
            dead: bucket.dead,
            oldest_pending_minutes: pending_minutes(bucket.oldest, now)?,
        });
    }
    Ok(queues)
}

fn add_job(
    bucket: &mut Bucket,
    state: &str,
    run_at: &str,
    created_at: &str,
    now: DateTime<Utc>,
) -> Result<(), SqliteError> {
    let state = state
        .parse::<JobState>()
        .map_err(|_| SqliteError::StoredState {
            value: state.to_owned(),
        })?;
    match state {
        JobState::Running => bucket.running += 1,
        JobState::Dead => bucket.dead += 1,
        JobState::Queued => note_queued(bucket, run_at, created_at, now)?,
    }
    Ok(())
}

fn note_queued(
    bucket: &mut Bucket,
    run_at: &str,
    created_at: &str,
    now: DateTime<Utc>,
) -> Result<(), SqliteError> {
    let due = time::stored(run_at, "run_at")?;
    if due > now {
        bucket.scheduled += 1;
        return Ok(());
    }
    bucket.pending += 1;
    let created = time::stored(created_at, "created_at")?;
    bucket.oldest = Some(match bucket.oldest {
        Some(current) if current <= created => current,
        Some(_) | None => created,
    });
    Ok(())
}

fn pending_minutes(
    oldest: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Result<Option<u64>, SqliteError> {
    let Some(oldest) = oldest else {
        return Ok(None);
    };
    let minutes = (now - oldest).num_minutes();
    u64::try_from(minutes)
        .map(Some)
        .map_err(|_| SqliteError::Clock)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;
    use chrono::TimeZone as _;
    use chrono::Utc;
    use wire::gateway::memory::EmbedStatus;
    use wire::gateway::memory::MemoryHealthStatus;

    use crate::SqliteStore;
    use crate::queue::JobInsert;

    fn at() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 10, 12, 0, 0).unwrap()
    }

    #[tokio::test]
    async fn queue_health_counts_each_bucket() {
        let store = SqliteStore::open_in_memory().unwrap();
        let now = at();
        store
            .enqueue_job(
                JobInsert::new("t", "null".to_owned(), now)
                    .key("later")
                    .run_at(now + Duration::hours(1)),
            )
            .await
            .unwrap();
        store
            .enqueue_job(
                JobInsert::new("t", "null".to_owned(), now)
                    .key("dead-one")
                    .max_attempts(1),
            )
            .await
            .unwrap();
        store
            .enqueue_job(JobInsert::new("t", "null".to_owned(), now).key("running"))
            .await
            .unwrap();
        store
            .enqueue_job(JobInsert::new("t", "null".to_owned(), now).key("pending"))
            .await
            .unwrap();

        let first = store
            .claim_tasks(vec!["t".to_owned()], now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.key.as_deref(), Some("dead-one"));
        store
            .fail_job(first.id, "boom".to_owned(), now)
            .await
            .unwrap();
        let second = store
            .claim_tasks(vec!["t".to_owned()], now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.key.as_deref(), Some("running"));

        let report = store.health(now).await.unwrap();
        assert_eq!(report.status, MemoryHealthStatus::Degraded);
        assert_eq!(report.embeddings.status, EmbedStatus::Unavailable);
        assert_eq!(report.embed_queue_depth, 0);
        let queue = report.queues.first().unwrap();
        assert_eq!(report.queues.len(), 1);
        assert_eq!(queue.task, "t");
        assert_eq!(queue.pending, 1);
        assert_eq!(queue.running, 1);
        assert_eq!(queue.scheduled, 1);
        assert_eq!(queue.dead, 1);
        assert_eq!(queue.oldest_pending_minutes, Some(0));
    }
}
