use std::str::FromStr as _;

use async_trait::async_trait;
use chrono::Utc;
use kernel::queue::ClaimedJob;
use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use wire::JsonObject;

use crate::SqliteStore;
use crate::error::SqliteError;
use crate::queue::FailOutcome;
use crate::queue::JobInsert;
use crate::queue::JobPayload;

#[async_trait]
impl JobQueue for SqliteStore {
    async fn enqueue(&self, job: Job) -> Result<(), JobQueueError> {
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
        let insert = JobInsert {
            task: job.name.to_string(),
            key: job.key.clone(),
            payload: payload.clone(),
            run_at,
            max_attempts: port_max_attempts(&job.name),
            now,
        };
        let inserted = self.enqueue_job(insert).await.map_err(unavailable)?;
        if inserted {
            return Ok(());
        }
        match (job.key, job.key_mode) {
            (Some(key), JobKeyMode::Replace) => self
                .replace_job(key, payload, run_at, now)
                .await
                .map_err(unavailable),
            (Some(_), JobKeyMode::KeepRunAt) => Ok(()),
            (None, JobKeyMode::KeepRunAt | JobKeyMode::Replace) => {
                Err(unavailable(SqliteError::InsertRejected))
            }
        }
    }

    async fn claim(&self, role: WorkerRole) -> Result<Option<ClaimedJob>, JobQueueError> {
        let tasks = tasks_for(role)
            .iter()
            .map(|task| (*task).to_owned())
            .collect();
        match self.claim_port(tasks, Utc::now()).await {
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
        let text = self.text_for_claim(id).map_err(unavailable)?;
        self.complete_job(text).await.map_err(unavailable)
    }

    async fn fail(&self, id: JobId, error: &str) -> Result<(), JobQueueError> {
        let text = self.text_for_claim(id).map_err(unavailable)?;
        let outcome = self
            .fail_job(text, error.to_owned(), Utc::now())
            .await
            .map_err(unavailable)?;
        match outcome {
            FailOutcome::Retry | FailOutcome::Dead => Ok(()),
        }
    }
}

fn unavailable(error: SqliteError) -> JobQueueError {
    JobQueueError::Unavailable(Box::new(error))
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
        | SqliteError::InvalidTimestamp { .. }
        | SqliteError::Query { .. }
        | SqliteError::Encode { .. }
        | SqliteError::StoredJson { .. }
        | SqliteError::StoredTimestamp { .. }
        | SqliteError::Count(_)
        | SqliteError::MissingJob { .. }
        | SqliteError::Clock
        | SqliteError::NoHandler { .. }
        | SqliteError::StoredState { .. }
        | SqliteError::StoredDims { .. }
        | SqliteError::EmbedAlreadyQueued { .. }
        | SqliteError::InsertRejected
        | SqliteError::Embed { .. }
        | SqliteError::MissingEmbedTarget { .. }
        | SqliteError::StoredRole { .. } => None,
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

fn tasks_for(role: WorkerRole) -> &'static [&'static str] {
    match role {
        WorkerRole::Embedder => &["embed-target"],
        WorkerRole::Summarizer => &[
            "compact-conversation",
            "synthesize-tool-call",
            "enqueue-idle",
            "enqueue-stale-compaction",
            "reap-dead-jobs",
        ],
        WorkerRole::Categorizer => &["suggest-tags"],
        WorkerRole::WikiBuilder => &[
            "extract-wiki",
            "enqueue-wiki-backfill",
            "consolidate-wiki",
            "recompile-wiki",
            "enqueue-stale-wiki",
        ],
    }
}

fn port_max_attempts(name: &JobName) -> u32 {
    match name {
        JobName::Fixed(fixed) => fixed_attempts(*fixed),
        JobName::Heartbeat { .. } | JobName::RunTaskOn { .. } => 1,
    }
}

fn fixed_attempts(fixed: FixedJob) -> u32 {
    match fixed {
        FixedJob::CompactConversation | FixedJob::SynthesizeToolCall => 3,
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
        | FixedJob::EnqueueStaleCompaction
        | FixedJob::ReapDeadJobs => 5,
    }
}
