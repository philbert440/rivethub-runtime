use crate::error::Error;
use crate::graphile::GraphileQueue;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::queue::ClaimSelector;
use kernel::queue::ClaimedJob;
use kernel::queue::FixedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use serde::Deserialize;
use serde::Serialize;
use sqlx::PgPool;

#[derive(Debug)]
pub struct PostgresQueue {
    inner: GraphileQueue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronEntry {
    pub identifier: String,
    pub task: JobName,
    pub schedule: String,
    pub backfill_seconds: u64,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
            Ok(Self {
                inner: GraphileQueue::new(pool),
            })
        } else {
            Err(Error::QueueAbsent)
        }
    }

    #[must_use]
    pub fn task_list(role: WorkerRole) -> &'static [JobName] {
        match role {
            WorkerRole::Embedder => &[JobName::Fixed(FixedJob::EmbedTarget)],
            WorkerRole::Summarizer => &[
                JobName::Fixed(FixedJob::SummarizeConversation),
                JobName::Fixed(FixedJob::SynthesizeToolCall),
                JobName::Fixed(FixedJob::EnqueueIdle),
                JobName::Fixed(FixedJob::EnqueueStaleSummaries),
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
                    JobName::Fixed(FixedJob::EnqueueStaleSummaries),
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

    pub async fn dead_job_ids(&self, task: &JobName, limit: u32) -> Result<Vec<JobId>, Error> {
        self.inner.dead_job_ids(task, limit).await
    }

    pub async fn reschedule_dead(
        &self,
        task: &JobName,
        limit: u32,
        priority: i16,
    ) -> Result<u64, Error> {
        self.inner.reschedule_dead(task, limit, priority).await
    }

    pub async fn reap_dead_corpses(&self, allowlist: &[JobName], limit: u32) -> Result<u64, Error> {
        self.inner.reap_dead_corpses(allowlist, limit).await
    }

    pub async fn queue_health(&self, task: &JobName) -> Result<Option<QueueHealth>, Error> {
        self.inner.queue_health(task).await
    }
}

#[async_trait]
impl JobQueue for PostgresQueue {
    fn task_names(&self, role: WorkerRole) -> Vec<JobName> {
        Self::task_list(role).to_vec()
    }

    async fn subscribe(&self) -> Result<Box<dyn kernel::queue::JobNotifications>, JobQueueError> {
        self.inner.subscribe().await.map_err(JobQueueError::from)
    }

    async fn enqueue(&self, job: Job) -> Result<JobId, JobQueueError> {
        self.inner
            .enqueue(&job)
            .await
            .map_err(|source| JobQueueError::Unavailable(Box::new(source)))
    }

    async fn claim(&self, selector: &ClaimSelector) -> Result<Option<ClaimedJob>, JobQueueError> {
        self.inner
            .claim(&selector.names, selector.only_id)
            .await
            .map_err(JobQueueError::from)
    }

    async fn complete(&self, id: JobId) -> Result<(), JobQueueError> {
        self.inner
            .complete(id)
            .await
            .map(|_| ())
            .map_err(|source| JobQueueError::Unavailable(Box::new(source)))
    }

    async fn fail(&self, id: JobId, error: &str) -> Result<(), JobQueueError> {
        self.inner
            .fail(id, error)
            .await
            .map_err(|source| JobQueueError::Unavailable(Box::new(source)))
    }

    async fn release(&self, id: JobId) -> Result<(), JobQueueError> {
        match self.inner.release(id).await {
            Ok(0) => Err(JobQueueError::Missing { id }),
            Ok(_) => Ok(()),
            Err(source) => Err(JobQueueError::Unavailable(Box::new(source))),
        }
    }

    async fn revive_dead(
        &self,
        name: &JobName,
        dead_before: DateTime<Utc>,
    ) -> Result<u64, JobQueueError> {
        self.inner
            .revive_before(name, dead_before)
            .await
            .map_err(JobQueueError::from)
    }

    async fn read(&self, id: JobId) -> Result<Option<kernel::queue::StoredJob>, JobQueueError> {
        self.inner.read(id).await
    }
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
