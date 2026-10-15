use std::str::FromStr as _;

use async_trait::async_trait;
use chrono::Utc;
use kernel::queue::ClaimSelector;
use kernel::queue::ClaimedJob;
use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::StoredJob;
use kernel::queue::WorkerRole;
use rusqlite::OptionalExtension as _;
use wire::JsonObject;

use crate::SqliteStore;
use crate::error::SqliteError;
use crate::queue::FailOutcome;
use crate::queue::JobInsert;
use crate::queue::JobPayload;

#[async_trait]
impl JobQueue for SqliteStore {
    fn task_names(&self, role: WorkerRole) -> Vec<JobName> {
        tasks_for(role)
    }

    async fn subscribe(&self) -> Result<Box<dyn kernel::queue::JobNotifications>, JobQueueError> {
        Ok(Box::new(self.notifications.subscribe()))
    }

    async fn enqueue(&self, job: Job) -> Result<JobId, JobQueueError> {
        let payload = serde_json::to_string(&job.payload).map_err(|source| {
            unavailable(SqliteError::Encode {
                column: "payload",
                source,
            })
        })?;
        let now = Utc::now();
        let run_at = match job.run_at {
            Some(at) => at,
            None => now,
        };
        let max_attempts = resolved_attempts(&job)?;
        let insert = JobInsert {
            task: job.name.to_string(),
            key: job.key.clone(),
            payload: payload.clone(),
            run_at,
            max_attempts,
            now,
        };
        let inserted = self.enqueue_returning(insert).await.map_err(unavailable)?;
        let text = match inserted {
            Some(id) => id,
            None => self.existing_key(&job, payload, run_at, now).await?,
        };
        self.remember_claim(&text).map_err(unavailable)
    }

    async fn claim(&self, selector: &ClaimSelector) -> Result<Option<ClaimedJob>, JobQueueError> {
        let only = match selector.only_id {
            Some(id) => match self.text_for_claim(id) {
                Ok(text) => Some(text),
                Err(SqliteError::MissingJob { .. }) => return Ok(None),
                Err(error) => return Err(unavailable(error)),
            },
            None => None,
        };
        let tasks = selector.names.iter().map(ToString::to_string).collect();
        match self.claim_port(tasks, only, Utc::now()).await {
            Ok(None) => Ok(None),
            Ok(Some(job)) => Ok(Some(self.claimed_port(job)?)),
            Err(error) => match claim_message(&error) {
                Some(message) => Err(JobQueueError::Claim {
                    message,
                    source: Box::new(error),
                }),
                None => Err(unavailable(error)),
            },
        }
    }

    async fn complete(&self, id: JobId) -> Result<(), JobQueueError> {
        let text = self
            .text_for_claim(id)
            .map_err(|error| missing(id, error))?;
        self.complete_job(text)
            .await
            .map_err(|error| missing(id, error))
    }

    async fn fail(&self, id: JobId, error: &str) -> Result<(), JobQueueError> {
        let text = self
            .text_for_claim(id)
            .map_err(|error| missing(id, error))?;
        let outcome = self
            .fail_job(text, error.to_owned(), Utc::now())
            .await
            .map_err(|error| missing(id, error))?;
        match outcome {
            FailOutcome::Retry | FailOutcome::Dead => Ok(()),
        }
    }

    async fn revive_dead(
        &self,
        name: &JobName,
        dead_before: chrono::DateTime<Utc>,
    ) -> Result<u64, JobQueueError> {
        let task = name.to_string();
        let run_at = crate::time::stamp(Utc::now());
        let cutoff = crate::time::stamp(dead_before);
        self.with_conn(move |conn| revive_dead_jobs(conn, &task, &run_at, &cutoff))
            .await
            .map_err(unavailable)
    }

    async fn read(&self, id: JobId) -> Result<Option<StoredJob>, JobQueueError> {
        let text = match self.text_for_claim(id) {
            Ok(text) => text,
            Err(SqliteError::MissingJob { .. }) => return Ok(None),
            Err(error) => return Err(unavailable(error)),
        };
        let row = self
            .with_conn(move |conn| read_stored(conn, &text))
            .await
            .map_err(unavailable)?;
        let Some(row) = row else {
            return Ok(None);
        };
        Ok(Some(stored_job(id, row)?))
    }
}

impl SqliteStore {
    async fn existing_key(
        &self,
        job: &Job,
        payload: String,
        run_at: chrono::DateTime<Utc>,
        now: chrono::DateTime<Utc>,
    ) -> Result<String, JobQueueError> {
        let Some(key) = job.key.clone() else {
            return Err(unavailable(SqliteError::InsertRejected));
        };
        if job.key_mode == JobKeyMode::Replace {
            self.replace_job(key.clone(), payload, run_at, now)
                .await
                .map_err(unavailable)?;
        }
        let key_for_lookup = key;
        self.with_conn(move |conn| id_for_key(conn, &key_for_lookup))
            .await
            .map_err(unavailable)
    }
}

fn revive_dead_jobs(
    conn: &rusqlite::Connection,
    task: &str,
    run_at: &str,
    cutoff: &str,
) -> Result<u64, SqliteError> {
    let changed = conn
        .execute(
            "UPDATE ros_jobs
                SET state = 'queued', attempts = 0, run_at = ?2,
                    locked_at = NULL, last_error = NULL, updated_at = ?2
              WHERE task = ?1 AND state = 'dead' AND updated_at < ?3",
            rusqlite::params![task, run_at, cutoff],
        )
        .map_err(SqliteError::query("revive dead jobs"))?;
    u64::try_from(changed).map_err(SqliteError::Count)
}

fn unavailable(error: SqliteError) -> JobQueueError {
    JobQueueError::Unavailable(Box::new(error))
}

fn missing(id: JobId, error: SqliteError) -> JobQueueError {
    if let SqliteError::MissingJob { .. } = error {
        JobQueueError::Missing { id }
    } else {
        unavailable(error)
    }
}

fn resolved_attempts(job: &Job) -> Result<u32, JobQueueError> {
    match job.max_attempts {
        Some(attempts) => {
            u32::try_from(attempts).map_err(|_| JobQueueError::Stored("max_attempts".to_owned()))
        }
        None => Ok(port_max_attempts(&job.name)),
    }
}

struct StoredRow {
    task: String,
    key: Option<String>,
    payload: String,
    max_attempts: i64,
    attempts: i64,
    last_error: Option<String>,
}

fn read_stored(conn: &rusqlite::Connection, id: &str) -> Result<Option<StoredRow>, SqliteError> {
    conn.query_row(
        "SELECT task, job_key, payload, max_attempts, attempts, last_error FROM ros_jobs WHERE id = ?1",
        [id],
        |row| {
            Ok(StoredRow {
                task: row.get(0)?,
                key: row.get(1)?,
                payload: row.get(2)?,
                max_attempts: row.get(3)?,
                attempts: row.get(4)?,
                last_error: row.get(5)?,
            })
        },
    )
    .optional()
    .map_err(SqliteError::query("job read"))
}

fn id_for_key(conn: &rusqlite::Connection, key: &str) -> Result<String, SqliteError> {
    conn.query_row("SELECT id FROM ros_jobs WHERE job_key = ?1", [key], |row| {
        row.get(0)
    })
    .map_err(SqliteError::query("job id"))
}

fn stored_job(id: JobId, row: StoredRow) -> Result<StoredJob, JobQueueError> {
    let name =
        JobName::from_str(&row.task).map_err(|error| JobQueueError::Stored(error.to_string()))?;
    let payload = serde_json::from_str(&row.payload)
        .map_err(|source| JobQueueError::Payload(Box::new(source)))?;
    let object = match payload {
        JobPayload::Object(object) => object,
        JobPayload::Text(_) | JobPayload::Absent => {
            return Err(JobQueueError::Payload(Box::new(
                SqliteError::PayloadNotObject,
            )));
        }
    };
    Ok(StoredJob {
        id,
        name,
        payload: object,
        key: row.key,
        priority: None,
        max_attempts: i32::try_from(row.max_attempts)
            .map_err(|_| JobQueueError::Stored("max_attempts".to_owned()))?,
        attempts: u32::try_from(row.attempts)
            .map_err(|_| JobQueueError::Stored("attempts".to_owned()))?,
        last_error: row.last_error,
    })
}

fn claim_message(error: &SqliteError) -> Option<String> {
    match error {
        SqliteError::PayloadNotObject => Some("job payload is not a JSON object".to_owned()),
        SqliteError::ClaimRejected { message } => Some(message.clone()),
        SqliteError::StoredPayload { source } => Some(source.to_string()),
        SqliteError::Open { .. }
        | SqliteError::NotInitialised { .. }
        | SqliteError::SchemaBehind { .. }
        | SqliteError::Pragma(_)
        | SqliteError::UnsupportedSchema { .. }
        | SqliteError::Join(_)
        | SqliteError::Query { .. }
        | SqliteError::Encode { .. }
        | SqliteError::StoredJson { .. }
        | SqliteError::StoredTimestamp { .. }
        | SqliteError::Count(_)
        | SqliteError::MissingJob { .. }
        | SqliteError::Clock
        | SqliteError::StoredState { .. }
        | SqliteError::EmbedAlreadyQueued { .. }
        | SqliteError::InsertRejected
        | SqliteError::InvertedRange
        | SqliteError::StoredTagType { .. }
        | SqliteError::Regex(_)
        | SqliteError::StoredEmbedding
        | SqliteError::SearchCancelled
        | SqliteError::SynthesisUnconfigured
        | SqliteError::MissingHit { .. }
        | SqliteError::MissingTable { .. }
        | SqliteError::TagTablesMissing
        | SqliteError::Invalid { .. }
        | SqliteError::Width { .. } => None,
    }
}

impl SqliteStore {
    fn claimed_port(
        &self,
        job: crate::queue::ClaimedSqliteJob,
    ) -> Result<ClaimedJob, JobQueueError> {
        let name = JobName::from_str(&job.task).map_err(|error| JobQueueError::Claim {
            message: error.to_string(),
            source: Box::new(error),
        })?;
        let payload = object_payload(job.payload)?;
        let id = self.remember_claim(&job.id).map_err(unavailable)?;
        Ok(ClaimedJob {
            id,
            name,
            payload,
            attempt: job.attempts,
        })
    }
}

fn object_payload(payload: JobPayload) -> Result<JsonObject, JobQueueError> {
    match payload {
        JobPayload::Object(object) => Ok(object),
        JobPayload::Text(_) | JobPayload::Absent => Err(JobQueueError::Claim {
            message: "job payload is not a JSON object".to_owned(),
            source: Box::new(SqliteError::PayloadNotObject),
        }),
    }
}

fn tasks_for(role: WorkerRole) -> Vec<JobName> {
    match role {
        WorkerRole::Embedder => vec![fixed(FixedJob::EmbedTarget)],
        WorkerRole::Summarizer => vec![
            fixed(FixedJob::SummarizeConversation),
            fixed(FixedJob::SynthesizeToolCall),
            fixed(FixedJob::EnqueueIdle),
            fixed(FixedJob::EnqueueStaleSummaries),
            fixed(FixedJob::ReapDeadJobs),
        ],
        WorkerRole::Categorizer => vec![fixed(FixedJob::SuggestTags)],
        WorkerRole::WikiBuilder => vec![
            fixed(FixedJob::ExtractWiki),
            fixed(FixedJob::EnqueueWikiBackfill),
            fixed(FixedJob::ConsolidateWiki),
            fixed(FixedJob::RecompileWiki),
            fixed(FixedJob::EnqueueStaleWiki),
        ],
    }
}

fn fixed(job: FixedJob) -> JobName {
    JobName::Fixed(job)
}

fn port_max_attempts(name: &JobName) -> u32 {
    match name {
        JobName::Fixed(fixed) => fixed_attempts(*fixed),
        JobName::Heartbeat { .. } | JobName::RunTaskOn { .. } => 1,
    }
}

fn fixed_attempts(fixed: FixedJob) -> u32 {
    match fixed {
        FixedJob::SummarizeConversation | FixedJob::SynthesizeToolCall => 3,
        FixedJob::SuggestTags | FixedJob::ExtractWiki => 2,
        FixedJob::RunTask => 1,
        FixedJob::EmbedTarget
        | FixedJob::EnqueueUnembedded
        | FixedJob::EnqueueUnchunked
        | FixedJob::EnqueueIdle
        | FixedJob::EnqueueWikiBackfill
        | FixedJob::ConsolidateWiki
        | FixedJob::RecompileWiki
        | FixedJob::EnqueueStaleWiki
        | FixedJob::EnqueueStaleSummaries
        | FixedJob::ReapDeadJobs => 5,
    }
}
