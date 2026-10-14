use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use kernel::memory::EmbeddingStore;
use kernel::model::ModelProvider;
use kernel::queue::ClaimedJob;
use kernel::queue::JobId;
use kernel::queue::JobQueue;
use kernel::queue::WorkerRole;
use tokio::sync::Mutex;
use tokio::sync::Notify;
use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::Semaphore;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::embed::EmbedSettings;
use crate::embed::handle_embed;
use crate::error::WorkerError;
use crate::role::ProcessRole;
use crate::role::handler_ready;

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
    reason = "the model provider port is a trait object without Debug"
)]
pub struct Runner<Q> {
    queue: Arc<Q>,
    targets: Arc<dyn EmbeddingStore>,
    provider: Arc<dyn ModelProvider>,
    config: RunnerConfig,
    embed: EmbedSettings,
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
    pub fn new(
        queue: Arc<Q>,
        targets: Arc<dyn EmbeddingStore>,
        provider: Arc<dyn ModelProvider>,
        config: RunnerConfig,
        embed: EmbedSettings,
    ) -> Self {
        let (idle, _) = watch::channel(false);
        Self {
            queue,
            targets,
            provider,
            config,
            embed,
            wake: Arc::new(Notify::new()),
            idle,
            cursor: Arc::new(AtomicUsize::new(0)),
            open: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn wake(&self) {
        self.wake.notify_one();
    }

    #[must_use]
    pub fn idle(&self) -> watch::Receiver<bool> {
        self.idle.subscribe()
    }

    pub async fn run(&self, cancel: CancellationToken) -> Result<(), WorkerError> {
        let roles = self.config.role.claim_roles()?;
        let permits =
            usize::try_from(self.config.concurrency).map_err(|_| WorkerError::Concurrency)?;
        let semaphore = Arc::new(Semaphore::new(permits));
        let mut tasks = JoinSet::new();
        let outcome = self
            .claim_loop(&roles, &semaphore, &mut tasks, &cancel)
            .await;
        let reaped = self.reap(&mut tasks, true).await;
        outcome.and(reaped)
    }

    async fn claim_loop(
        &self,
        roles: &[WorkerRole],
        semaphore: &Arc<Semaphore>,
        tasks: &mut JoinSet<Result<(), WorkerError>>,
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
                if !self.park(cancel).await {
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
        if count == 0 {
            return Ok(None);
        }
        let start = self.cursor.fetch_add(1, Ordering::Relaxed) % count;
        for role in roles.iter().cycle().skip(start).take(count) {
            if let Some(job) = self.queue.claim(*role).await.map_err(WorkerError::queue)? {
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
        let targets = Arc::clone(&self.targets);
        let provider = Arc::clone(&self.provider);
        let embed = self.embed;
        let wake = Arc::clone(&self.wake);
        let open = Arc::clone(&self.open);
        let id = job.id;
        tasks.spawn(async move {
            let result = settle(
                queue.as_ref(),
                targets.as_ref(),
                provider.as_ref(),
                embed,
                job,
                &cancel,
            )
            .await;
            {
                let mut running = open.lock().await;
                running.retain(|seen| *seen != id);
            }
            drop(permit);
            wake.notify_one();
            result
        });
    }

    async fn park(&self, cancel: &CancellationToken) -> bool {
        tokio::select! {
            biased;
            () = cancel.cancelled() => false,
            () = self.wake.notified() => true,
            () = tokio::time::sleep(self.config.poll_interval) => true,
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

async fn settle<Q>(
    queue: &Q,
    targets: &dyn EmbeddingStore,
    provider: &dyn ModelProvider,
    embed: EmbedSettings,
    job: ClaimedJob,
    cancel: &CancellationToken,
) -> Result<(), WorkerError>
where
    Q: JobQueue,
{
    let id = job.id;
    let handled = dispatch(targets, provider, embed, &job, cancel).await;
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

async fn dispatch(
    targets: &dyn EmbeddingStore,
    provider: &dyn ModelProvider,
    embed: EmbedSettings,
    job: &ClaimedJob,
    cancel: &CancellationToken,
) -> Result<(), WorkerError> {
    if !handler_ready(&job.name) {
        return Err(WorkerError::Unhandled {
            name: job.name.to_string(),
        });
    }
    handle_embed(targets, provider, embed, &job.payload, cancel).await
}
