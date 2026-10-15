use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use chrono::Utc;
use kernel::memory::EmbeddingStore;
use kernel::memory::MemoryStore;
use kernel::memory::SummaryStore;
use kernel::model::ModelProvider;
use kernel::queue::ClaimedJob;
use kernel::queue::FixedJob;
use kernel::queue::JobId;
use kernel::queue::JobName;
use kernel::queue::JobNotifications;
use kernel::queue::JobQueue;
use kernel::queue::WorkerRole;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::Semaphore;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::categorize::CategorizeEnv;
use crate::categorize::TagSettings;
use crate::categorize::handle_suggest;
use crate::embed::EmbedSettings;
use crate::embed::handle_embed;
use crate::error::WorkerError;
use crate::role::ProcessRole;
use crate::summarize::SummarizeEnv;
use crate::summarize::SummarySettings;
use crate::summarize::handle_enqueue_idle;
use crate::summarize::handle_enqueue_stale;
use crate::summarize::handle_reap;
use crate::summarize::handle_summarize;
use crate::wiki_builder::{
    WikiBuilderDeps, WikiBuilderEnv, handle_extract_wiki, handle_wiki_backfill,
};

pub const EMBED_CONCURRENCY: u32 = 4;
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunnerConfig {
    pub role: ProcessRole,
    pub concurrency: u32,
    pub poll_interval: Duration,
}

impl RunnerConfig {
    pub fn new(role: ProcessRole, concurrency: u32) -> Result<Self, WorkerError> {
        if concurrency == 0 {
            return Err(WorkerError::Concurrency);
        }
        Ok(Self {
            role,
            concurrency,
            poll_interval: POLL_INTERVAL,
        })
    }

    #[must_use]
    pub fn embedder() -> Self {
        Self {
            role: ProcessRole::Embedder,
            concurrency: EMBED_CONCURRENCY,
            poll_interval: POLL_INTERVAL,
        }
    }
}

#[expect(
    missing_debug_implementations,
    reason = "the model provider and the summary and embedding stores are trait objects"
)]
pub struct WorkerDeps {
    pub targets: Arc<dyn EmbeddingStore>,
    pub summaries: Arc<dyn SummaryStore>,
    pub memory: Arc<dyn MemoryStore>,
    pub provider: Arc<dyn ModelProvider>,
    pub embed: EmbedSettings,
    pub summary: SummarySettings,
    pub tags: TagSettings,
}

#[expect(
    missing_debug_implementations,
    reason = "the model provider and the summary and embedding stores are trait objects"
)]
pub struct Runner<Q> {
    wiki: Option<WikiBuilderDeps>,
    queue: Arc<Q>,
    targets: Arc<dyn EmbeddingStore>,
    summaries: Arc<dyn SummaryStore>,
    memory: Arc<dyn MemoryStore>,
    provider: Arc<dyn ModelProvider>,
    config: RunnerConfig,
    embed: EmbedSettings,
    summary: SummarySettings,
    tags: TagSettings,
    wake: Arc<Notify>,
    idle: watch::Sender<bool>,
    cursor: Arc<AtomicUsize>,
    open: Arc<Mutex<Vec<JobId>>>,
}

impl<Q> Runner<Q>
where
    Q: JobQueue + 'static,
{
    #[must_use]
    pub fn new(queue: Arc<Q>, config: RunnerConfig, deps: WorkerDeps) -> Self {
        let (idle, _) = watch::channel(false);
        Self {
            wiki: None,
            queue,
            targets: deps.targets,
            summaries: deps.summaries,
            memory: deps.memory,
            provider: deps.provider,
            config,
            embed: deps.embed,
            summary: deps.summary,
            tags: deps.tags,
            wake: Arc::new(Notify::new()),
            idle,
            cursor: Arc::new(AtomicUsize::new(0)),
            open: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[must_use]
    pub fn with_wiki_builder(mut self, deps: WikiBuilderDeps) -> Self {
        self.wiki = Some(deps);
        self
    }

    #[must_use]
    pub fn idle(&self) -> watch::Receiver<bool> {
        self.idle.subscribe()
    }

    pub async fn run(&self, cancel: CancellationToken) -> Result<(), WorkerError> {
        let roles = self.config.role.claim_roles()?;
        if self.wiki.is_none() && roles.contains(&WorkerRole::WikiBuilder) {
            return Err(WorkerError::MissingDependencies {
                role: ProcessRole::WikiBuilder,
            });
        }
        let permits =
            usize::try_from(self.config.concurrency).map_err(|_| WorkerError::Concurrency)?;
        let semaphore = Arc::new(Semaphore::new(permits));
        let cancel = cancel.child_token();
        let mut tasks = JoinSet::new();
        let mut notifications = tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(()),
            result = self.queue.subscribe() => result.map_err(WorkerError::queue)?,
        };
        let outcome = self
            .claim_loop(
                &roles,
                &semaphore,
                &mut tasks,
                notifications.as_mut(),
                &cancel,
            )
            .await;
        cancel.cancel();
        let reaped = self.reap(&mut tasks, true).await;
        outcome.and(reaped)
    }

    async fn claim_loop(
        &self,
        roles: &[WorkerRole],
        semaphore: &Arc<Semaphore>,
        tasks: &mut JoinSet<Result<(), WorkerError>>,
        notifications: &mut dyn JobNotifications,
        cancel: &CancellationToken,
    ) -> Result<(), WorkerError> {
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            self.reap(tasks, false).await?;
            let Some(permit) = self.acquire(semaphore, cancel).await? else {
                return Ok(());
            };
            let Some(job) = self.claim_one(roles).await? else {
                drop(permit);
                self.publish_idle(tasks.is_empty());
                if !self.park(notifications, cancel).await? {
                    return Ok(());
                }
                continue;
            };
            self.note_open(job.id).await;
            self.spawn_job(tasks, permit, job, cancel.clone());
        }
    }

    async fn note_open(&self, id: JobId) {
        self.open.lock().await.push(id);
    }

    async fn acquire(
        &self,
        semaphore: &Arc<Semaphore>,
        cancel: &CancellationToken,
    ) -> Result<Option<OwnedSemaphorePermit>, WorkerError> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(None),
            result = semaphore.clone().acquire_owned() => result.map(Some).map_err(|_| WorkerError::Closed),
        }
    }

    async fn claim_one(&self, roles: &[WorkerRole]) -> Result<Option<ClaimedJob>, WorkerError> {
        let count = roles.len();
        let start = self.cursor.fetch_add(1, Ordering::Relaxed) % count;
        for role in roles.iter().cycle().skip(start).take(count) {
            let selector = kernel::queue::ClaimSelector::matching(self.queue.task_names(*role));
            if let Some(job) = self
                .queue
                .claim(&selector)
                .await
                .map_err(WorkerError::queue)?
            {
                return Ok(Some(job));
            }
        }
        Ok(None)
    }

    fn spawn_job(
        &self,
        tasks: &mut JoinSet<Result<(), WorkerError>>,
        permit: OwnedSemaphorePermit,
        job: ClaimedJob,
        cancel: CancellationToken,
    ) {
        self.publish_idle(false);
        let queue = Arc::clone(&self.queue);
        let env = JobEnv {
            wiki: self.wiki.clone(),
            targets: Arc::clone(&self.targets),
            summaries: Arc::clone(&self.summaries),
            memory: Arc::clone(&self.memory),
            provider: Arc::clone(&self.provider),
            embed: self.embed,
            summary: self.summary.clone(),
            tags: self.tags.clone(),
        };
        let wake = Arc::clone(&self.wake);
        let open = Arc::clone(&self.open);
        let id = job.id;
        tasks.spawn(async move {
            let result = settle(queue.as_ref(), &env, job, &cancel).await;
            {
                let mut running = open.lock().await;
                running.retain(|seen| *seen != id);
            }
            drop(permit);
            wake.notify_one();
            result
        });
    }

    async fn park(
        &self,
        notifications: &mut dyn JobNotifications,
        cancel: &CancellationToken,
    ) -> Result<bool, WorkerError> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Ok(false),
            notification = notifications.recv() => notification.map(|()| true).map_err(WorkerError::queue),
            () = self.wake.notified() => Ok(true),
            () = tokio::time::sleep(self.config.poll_interval) => Ok(true),
        }
    }

    fn publish_idle(&self, idle: bool) {
        let _previous = self.idle.send_replace(idle);
    }

    async fn reap(
        &self,
        tasks: &mut JoinSet<Result<(), WorkerError>>,
        wait: bool,
    ) -> Result<(), WorkerError> {
        let mut first = None;
        loop {
            let joined = if wait {
                tasks.join_next().await
            } else if let Some(joined) = tasks.try_join_next() {
                Some(joined)
            } else {
                break;
            };
            let Some(joined) = joined else {
                break;
            };
            let error = match joined {
                Ok(Ok(())) => None,
                Ok(Err(error)) => Some(error),
                Err(error) => Some(WorkerError::TaskEnded(error.to_string())),
            };
            let failed = error.is_some();
            if let Some(error) = error {
                first.get_or_insert(error);
            }
            if failed && !wait {
                break;
            }
        }
        if wait
            && let Err(error) = self.fail_open().await
            && first.is_none()
        {
            first = Some(error);
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    async fn fail_open(&self) -> Result<(), WorkerError> {
        let pending = {
            let mut open = self.open.lock().await;
            std::mem::take(&mut *open)
        };
        let mut first = None;
        for id in pending {
            if let Err(error) = self.queue.fail(id, "a worker task ended").await
                && first.is_none()
            {
                first = Some(WorkerError::queue(error));
            }
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

struct JobEnv {
    wiki: Option<WikiBuilderDeps>,
    targets: Arc<dyn EmbeddingStore>,
    summaries: Arc<dyn SummaryStore>,
    memory: Arc<dyn MemoryStore>,
    provider: Arc<dyn ModelProvider>,
    embed: EmbedSettings,
    summary: SummarySettings,
    tags: TagSettings,
}

async fn settle<Q>(
    queue: &Q,
    env: &JobEnv,
    job: ClaimedJob,
    cancel: &CancellationToken,
) -> Result<(), WorkerError>
where
    Q: JobQueue,
{
    let id = job.id;
    let handled = dispatch(queue, env, &job, cancel).await;
    match handled {
        Ok(()) => queue.complete(id).await.map_err(WorkerError::queue),
        Err(error) => fail_job(queue, id, &error).await,
    }
}

async fn fail_job<Q: JobQueue>(
    queue: &Q,
    id: kernel::queue::JobId,
    error: &WorkerError,
) -> Result<(), WorkerError> {
    tracing::warn!("job {id} failed: {error}");
    queue
        .fail(id, &error.to_string())
        .await
        .map_err(WorkerError::queue)
}

async fn dispatch<Q: JobQueue>(
    queue: &Q,
    env: &JobEnv,
    job: &ClaimedJob,
    cancel: &CancellationToken,
) -> Result<(), WorkerError> {
    let summarize = SummarizeEnv {
        store: env.summaries.as_ref(),
        provider: env.provider.as_ref(),
        queue,
        settings: &env.summary,
        now: Utc::now(),
        cancel,
    };
    match &job.name {
        JobName::Fixed(FixedJob::ExtractWiki | FixedJob::EnqueueWikiBackfill) => {
            let wiki = env.wiki.as_ref().ok_or(WorkerError::MissingDependencies {
                role: ProcessRole::WikiBuilder,
            })?;
            let wiki_env = WikiBuilderEnv {
                store: wiki.store.as_ref(),
                provider: env.provider.as_ref(),
                queue,
                model: &wiki.model,
                now: Utc::now(),
                cancel,
            };
            if job.name == JobName::Fixed(FixedJob::ExtractWiki) {
                handle_extract_wiki(&wiki_env, &job.payload)
                    .await
                    .map(|_| ())
            } else {
                handle_wiki_backfill(&wiki_env).await.map(|_| ())
            }
        }
        JobName::Fixed(FixedJob::EmbedTarget) => {
            handle_embed(
                env.targets.as_ref(),
                env.provider.as_ref(),
                env.embed,
                &job.payload,
                cancel,
            )
            .await
        }
        JobName::Fixed(FixedJob::SummarizeConversation) => {
            handle_summarize(&summarize, &job.payload).await.map(|_| ())
        }
        JobName::Fixed(FixedJob::EnqueueIdle) => handle_enqueue_idle(&summarize).await.map(|_| ()),
        JobName::Fixed(FixedJob::EnqueueStaleSummaries) => {
            handle_enqueue_stale(&summarize).await.map(|_| ())
        }
        JobName::Fixed(FixedJob::ReapDeadJobs) => handle_reap(&summarize).await.map(|_| ()),
        JobName::Fixed(FixedJob::SuggestTags) => {
            let categorize = CategorizeEnv {
                store: env.memory.as_ref(),
                provider: env.provider.as_ref(),
                settings: &env.tags,
                now: Utc::now(),
                cancel,
                attempt: job.attempt,
            };
            handle_suggest(&categorize, &job.payload).await.map(|_| ())
        }
        JobName::Fixed(
            FixedJob::EnqueueUnembedded
            | FixedJob::EnqueueUnchunked
            | FixedJob::SynthesizeToolCall
            | FixedJob::ConsolidateWiki
            | FixedJob::RecompileWiki
            | FixedJob::EnqueueStaleWiki
            | FixedJob::RunTask,
        )
        | JobName::Heartbeat { .. }
        | JobName::RunTaskOn { .. } => Err(WorkerError::Unhandled {
            name: job.name.to_string(),
        }),
    }
}
