use crate::error::Error;
use crate::graphile::GraphileQueue;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::queue::ClaimedJob;
use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use sqlx::PgPool;

#[derive(Debug)]
pub enum PostgresQueue {
    Graphile(GraphileQueue),
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronEntry {
    pub identifier: String,
    pub task: JobName,
    pub schedule: String,
    pub backfill_seconds: u64,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueHealth {
    pub task: JobName,
    pub pending: u64,
    pub running: u64,
    pub scheduled: u64,
    pub dead: u64,
    pub recent_dead: u64,
    pub oldest_pending_minutes: Option<u64>,
    pub last_error: Option<String>,
}

impl PostgresQueue {
    pub async fn connect(pool: PgPool) -> Result<Self, Error> {
        if GraphileQueue::schema_present(&pool).await? {
            Ok(Self::Graphile(GraphileQueue::new(pool)))
        } else {
            Ok(Self::Absent)
        }
    }

    #[must_use]
    pub fn available(&self) -> bool {
        matches!(self, Self::Graphile(_))
    }

    #[must_use]
    pub fn task_list(role: WorkerRole) -> &'static [JobName] {
        match role {
            WorkerRole::Embedder => &[JobName::Fixed(FixedJob::EmbedTarget)],
            WorkerRole::Summarizer => &[
                JobName::Fixed(FixedJob::CompactConversation),
                JobName::Fixed(FixedJob::SynthesizeToolCall),
                JobName::Fixed(FixedJob::EnqueueIdle),
                JobName::Fixed(FixedJob::EnqueueStaleCompaction),
                JobName::Fixed(FixedJob::ReapDeadJobs),
            ],
            WorkerRole::Categorizer => &[JobName::Fixed(FixedJob::SuggestTags)],
            WorkerRole::WikiBuilder => &[
                JobName::Fixed(FixedJob::ExtractWiki),
                JobName::Fixed(FixedJob::EnqueueWikiBackfill),
            ],
        }
    }

    #[must_use]
    pub fn crontab(role: WorkerRole) -> Vec<CronEntry> {
        match role {
            WorkerRole::Embedder => vec![
                cron(
                    "unembedded-sweep",
                    JobName::Fixed(FixedJob::EnqueueUnembedded),
                    "*/10 * * * *",
                ),
                cron(
                    "unchunked-sweep",
                    JobName::Fixed(FixedJob::EnqueueUnchunked),
                    "*/15 * * * *",
                ),
            ],
            WorkerRole::Summarizer => vec![
                cron(
                    "idle-enqueue",
                    JobName::Fixed(FixedJob::EnqueueIdle),
                    "*/5 * * * *",
                ),
                cron(
                    "stale-compaction-sweep",
                    JobName::Fixed(FixedJob::EnqueueStaleCompaction),
                    "*/15 * * * *",
                ),
                cron(
                    "reap-dead-jobs",
                    JobName::Fixed(FixedJob::ReapDeadJobs),
                    "0 * * * *",
                ),
            ],
            WorkerRole::Categorizer => vec![],
            WorkerRole::WikiBuilder => vec![cron(
                "wiki-backfill",
                JobName::Fixed(FixedJob::EnqueueWikiBackfill),
                "*/10 * * * *",
            )],
        }
    }

    #[must_use]
    pub fn heartbeat_schedule(interval: &str) -> Option<String> {
        let minutes = interval.parse::<u32>().ok().or_else(|| {
            interval
                .strip_suffix('m')
                .and_then(|value| value.parse::<u32>().ok())
        });
        if let Some(n) = minutes {
            return Some(format!("*/{n} * * * *"));
        }
        if let Some(h) = interval
            .strip_suffix('h')
            .and_then(|value| value.parse::<u32>().ok())
        {
            return Some(format!("0 */{h} * * *"));
        }
        (interval.split_whitespace().count() == 5).then(|| interval.to_string())
    }

    pub async fn dead_job_ids(&self, task: &JobName, limit: u32) -> Result<Vec<JobId>, Error> {
        match self {
            Self::Graphile(queue) => queue.dead_job_ids(task, limit).await,
            Self::Absent => Err(Error::QueueAbsent),
        }
    }

    pub async fn reschedule_dead(
        &self,
        task: &JobName,
        limit: u32,
        priority: i16,
    ) -> Result<u64, Error> {
        match self {
            Self::Graphile(queue) => queue.reschedule_dead(task, limit, priority).await,
            Self::Absent => Err(Error::QueueAbsent),
        }
    }

    pub async fn reap_dead_corpses(&self, allowlist: &[JobName], limit: u32) -> Result<u64, Error> {
        match self {
            Self::Graphile(queue) => queue.reap_dead_corpses(allowlist, limit).await,
            Self::Absent => Err(Error::QueueAbsent),
        }
    }

    pub async fn queue_health(&self, task: &JobName) -> Result<Option<QueueHealth>, Error> {
        match self {
            Self::Graphile(queue) => queue.queue_health(task).await,
            Self::Absent => Err(Error::QueueAbsent),
        }
    }
}

#[async_trait]
impl JobQueue for PostgresQueue {
    async fn enqueue(&self, job: Job) -> Result<(), JobQueueError> {
        match self {
            Self::Graphile(queue) => queue
                .enqueue(&job)
                .await
                .map(|_| ())
                .map_err(|source| JobQueueError::Unavailable(Box::new(source))),
            Self::Absent => Err(absent()),
        }
    }

    async fn claim(&self, role: WorkerRole) -> Result<Option<ClaimedJob>, JobQueueError> {
        match self {
            Self::Graphile(queue) => queue
                .claim(Self::task_list(role))
                .await
                .map_err(JobQueueError::from),
            Self::Absent => Err(absent()),
        }
    }

    async fn complete(&self, id: JobId) -> Result<(), JobQueueError> {
        match self {
            Self::Graphile(queue) => queue
                .complete(id)
                .await
                .map(|_| ())
                .map_err(|source| JobQueueError::Unavailable(Box::new(source))),
            Self::Absent => Err(absent()),
        }
    }

    async fn fail(&self, id: JobId, error: &str) -> Result<(), JobQueueError> {
        match self {
            Self::Graphile(queue) => queue
                .fail(id, error)
                .await
                .map_err(|source| JobQueueError::Unavailable(Box::new(source))),
            Self::Absent => Err(absent()),
        }
    }

    async fn revive_dead(
        &self,
        name: &JobName,
        dead_before: DateTime<Utc>,
    ) -> Result<u64, JobQueueError> {
        match self {
            Self::Graphile(queue) => queue
                .revive_before(name, dead_before)
                .await
                .map_err(JobQueueError::from),
            Self::Absent => Err(absent()),
        }
    }
}

fn absent() -> JobQueueError {
    JobQueueError::Unavailable(Box::new(Error::QueueAbsent))
}

fn cron(identifier: &str, task: JobName, schedule: &str) -> CronEntry {
    CronEntry {
        identifier: identifier.to_string(),
        task,
        schedule: schedule.to_string(),
        backfill_seconds: 0,
        max_attempts: 25,
    }
}
