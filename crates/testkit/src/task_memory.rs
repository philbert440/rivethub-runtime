use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::queue::ClaimSelector;
use kernel::queue::ClaimedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::StoredJob;
use kernel::queue::WorkerRole;
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::watch;
use uuid::Uuid;

mod query;

use kernel::queue::FixedJob;
use kernel::tasks::Finish;
use kernel::tasks::Task;
use kernel::tasks::TaskError;
use kernel::tasks::TaskFilter;
use kernel::tasks::TaskStatus;
use kernel::tasks::TaskStore;

#[derive(Debug)]
struct MemoryInner {
    tasks: BTreeMap<Uuid, Task>,
    jobs: BTreeMap<JobId, MemJob>,
    next_job: i64,
}

#[derive(Debug, Clone)]
struct MemJob {
    id: JobId,
    name: JobName,
    payload: wire::JsonObject,
    key: Option<String>,
    priority: i16,
    max_attempts: i32,
    attempts: i32,
    locked: bool,
    run_at: DateTime<Utc>,
    last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TaskMemory {
    inner: Arc<Mutex<MemoryInner>>,
    fail_enqueue: Arc<AtomicBool>,
    fail_list: Arc<AtomicBool>,
    generation: Arc<watch::Sender<u64>>,
    finished: broadcast::Sender<Uuid>,
    notifications: watch::Sender<()>,
}

impl TaskMemory {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MemoryInner {
                tasks: BTreeMap::new(),
                jobs: BTreeMap::new(),
                next_job: 1,
            })),
            fail_enqueue: Arc::new(AtomicBool::new(false)),
            fail_list: Arc::new(AtomicBool::new(false)),
            generation: Arc::new(watch::channel(0).0),
            finished: broadcast::channel(256).0,
            notifications: watch::channel(()).0,
        }
    }

    pub fn fail_next_enqueue(&self) {
        self.fail_enqueue.store(true, Ordering::SeqCst);
    }

    pub fn fail_next_list(&self) {
        self.fail_list.store(true, Ordering::SeqCst);
    }

    fn touch(&self) {
        self.generation.send_modify(|value| {
            *value = value.wrapping_add(1);
        });
    }

    fn changed_ids(&self, ids: Vec<Uuid>) {
        self.touch();
        for id in ids {
            if self.finished.send(id).is_err() {
                tracing::trace!("finished task has no listener");
            }
        }
    }
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "the next job id starts at 1; Default would zero it and the first enqueue would use id 0"
    )
)]
impl Default for TaskMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskStore for TaskMemory {
    async fn insert(&self, task: &Task) -> Result<(), TaskError> {
        let mut inner = self.inner.lock();
        if inner.tasks.contains_key(&task.id) {
            drop(inner);
            return Err(TaskError::Stored("duplicate task".to_string()));
        }
        inner.tasks.insert(task.id, task.clone());
        drop(inner);
        self.changed_ids(vec![task.id]);
        Ok(())
    }

    async fn load(&self, id: Uuid) -> Result<Task, TaskError> {
        self.inner
            .lock()
            .tasks
            .get(&id)
            .cloned()
            .ok_or(TaskError::Missing { id })
    }

    async fn list(&self, filter: &TaskFilter) -> Result<Vec<Task>, TaskError> {
        if self.fail_list.swap(false, Ordering::SeqCst) {
            return Err(TaskError::QueueAbsent);
        }
        let mut tasks: Vec<Task> = self
            .inner
            .lock()
            .tasks
            .values()
            .filter(|task| query::listed(task, filter))
            .cloned()
            .collect();
        tasks.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then(right.id.cmp(&left.id))
        });
        if let Some(limit) = filter.limit {
            tasks.truncate(
                usize::try_from(limit).map_err(|_| TaskError::Stored("limit".to_string()))?,
            );
        }
        Ok(tasks)
    }

    async fn by_child(&self, session: &str) -> Result<Option<Task>, TaskError> {
        Ok(self
            .inner
            .lock()
            .tasks
            .values()
            .find(|task| task.child.as_deref() == Some(session))
            .cloned())
    }

    async fn begin(&self, id: Uuid, node: &str, now: DateTime<Utc>) -> Result<bool, TaskError> {
        self.change(id, |task| {
            if task.status != TaskStatus::Queued || task.target.node != node {
                return false;
            }
            task.status = TaskStatus::Running;
            task.started_at = Some(now);
            task.alive_at = Some(now);
            true
        })
    }

    async fn attach(
        &self,
        id: Uuid,
        child: &str,
        harness: kernel::tasks::Harness,
    ) -> Result<(), TaskError> {
        self.change(id, |task| {
            task.child = Some(child.to_owned());
            task.target.harness = Some(harness);
            false
        })
        .map(drop)
    }

    async fn flip(&self, id: Uuid, status: TaskStatus) -> Result<bool, TaskError> {
        self.change(id, |task| {
            if !task.status.can_become(status) || status.terminal() {
                return false;
            }
            task.status = status;
            true
        })
    }

    async fn alive(&self, ids: &[Uuid], now: DateTime<Utc>) -> Result<(), TaskError> {
        let mut inner = self.inner.lock();
        for id in ids {
            if let Some(task) = inner.tasks.get_mut(id)
                && !task.status.terminal()
            {
                task.alive_at = Some(now);
            }
        }
        drop(inner);
        Ok(())
    }

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, TaskError> {
        self.change(id, |task| {
            if !task.status.can_become(finish.status) {
                return false;
            }
            task.status = finish.status;
            task.result.clone_from(&finish.result);
            task.error.clone_from(&finish.error);
            task.usage = finish.usage;
            task.completed_at = Some(finish.at);
            true
        })
    }

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, TaskError> {
        let mut prior = TaskStatus::Killed;
        self.change(id, |task| {
            prior = task.status;
            if task.status.terminal() {
                return false;
            }
            task.status = TaskStatus::Killed;
            task.completed_at = Some(now);
            true
        })?;
        Ok(prior)
    }

    async fn orphans(&self, node: &str, now: DateTime<Utc>) -> Result<Vec<Uuid>, TaskError> {
        Ok(self.fail_where(now, "its gateway restarted", |task| {
            task.target.node == node
        }))
    }

    async fn reap(
        &self,
        stale_before: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<Vec<Uuid>, TaskError> {
        Ok(self.fail_where(now, "its node stopped running it", |task| {
            task.alive_at.is_none_or(|alive| alive < stale_before)
        }))
    }

    async fn expire(&self, now: DateTime<Utc>) -> Result<Vec<Uuid>, TaskError> {
        let mut inner = self.inner.lock();
        let mut expired = Vec::new();
        for task in inner.tasks.values_mut() {
            if task.unclaimed_past_deadline(now) {
                task.status = TaskStatus::Timeout;
                task.error = Some(kernel::tasks::UNCLAIMED_PAST_DEADLINE.to_owned());
                task.completed_at = Some(now);
                expired.push(task.id);
            }
        }
        drop(inner);
        self.changed_ids(expired.clone());
        Ok(expired)
    }
}

impl TaskMemory {
    fn change(&self, id: Uuid, edit: impl FnOnce(&mut Task) -> bool) -> Result<bool, TaskError> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(TaskError::Missing { id })?;
        let changed = edit(task);
        drop(inner);
        if changed {
            self.changed_ids(vec![id]);
        }
        Ok(changed)
    }

    fn fail_where(
        &self,
        now: DateTime<Utc>,
        error: &str,
        pick: impl Fn(&Task) -> bool,
    ) -> Vec<Uuid> {
        let mut inner = self.inner.lock();
        let mut failed = Vec::new();
        for task in inner.tasks.values_mut() {
            if matches!(task.status, TaskStatus::Running | TaskStatus::AwaitingInput) && pick(task)
            {
                task.status = TaskStatus::Failed;
                task.error = Some(error.to_owned());
                task.completed_at = Some(now);
                failed.push(task.id);
            }
        }
        drop(inner);
        self.changed_ids(failed.clone());
        failed
    }
}

#[async_trait]
impl JobQueue for TaskMemory {
    fn task_names(&self, role: WorkerRole) -> Vec<JobName> {
        match role {
            WorkerRole::Embedder => vec![JobName::Fixed(FixedJob::EmbedTarget)],
            WorkerRole::Summarizer => vec![
                JobName::Fixed(FixedJob::SummarizeConversation),
                JobName::Fixed(FixedJob::SynthesizeToolCall),
                JobName::Fixed(FixedJob::EnqueueIdle),
                JobName::Fixed(FixedJob::EnqueueStaleSummaries),
                JobName::Fixed(FixedJob::ReapDeadJobs),
            ],
            WorkerRole::Categorizer => vec![JobName::Fixed(FixedJob::FileConversation)],
            WorkerRole::WikiBuilder => vec![
                JobName::Fixed(FixedJob::ExtractWiki),
                JobName::Fixed(FixedJob::EnqueueWikiBackfill),
            ],
        }
    }

    async fn subscribe(&self) -> Result<Box<dyn kernel::queue::JobNotifications>, JobQueueError> {
        Ok(Box::new(self.notifications.subscribe()))
    }

    async fn enqueue(&self, job: Job) -> Result<JobId, JobQueueError> {
        if self.fail_enqueue.swap(false, Ordering::SeqCst) {
            return Err(JobQueueError::Absent);
        }
        let Some(max_attempts) = job.max_attempts else {
            return Err(JobQueueError::Stored("max_attempts".to_owned()));
        };
        let mut inner = self.inner.lock();
        if let Some(existing) = inner
            .jobs
            .values_mut()
            .find(|item| item.key.is_some() && item.key == job.key)
        {
            existing.name = job.name;
            existing.payload = job.payload;
            existing.priority = job.priority;
            existing.max_attempts = max_attempts;
            if !existing.locked && job.key_mode == JobKeyMode::Replace {
                existing.attempts = 0;
                existing.last_error = None;
            }
            let id = existing.id;
            drop(inner);
            self.notifications.send_replace(());
            return Ok(id);
        }
        let raw = inner.next_job;
        inner.next_job = inner
            .next_job
            .checked_add(1)
            .ok_or_else(|| JobQueueError::Stored("job id overflow".to_owned()))?;
        let id = JobId::new(raw);
        inner.jobs.insert(
            id,
            MemJob {
                id,
                name: job.name,
                payload: job.payload,
                key: job.key,
                priority: job.priority,
                max_attempts,
                attempts: 0,
                locked: false,
                run_at: Utc::now(),
                last_error: None,
            },
        );
        drop(inner);
        self.notifications.send_replace(());
        Ok(id)
    }

    async fn claim(&self, selector: &ClaimSelector) -> Result<Option<ClaimedJob>, JobQueueError> {
        let mut inner = self.inner.lock();
        let mut choice: Option<JobId> = None;
        for job in inner.jobs.values() {
            if !selectable(job, selector) {
                continue;
            }
            choice = match choice {
                Some(current) => Some(prefer(&inner.jobs, current, job.id)),
                None => Some(job.id),
            };
        }
        let Some(id) = choice else {
            drop(inner);
            return Ok(None);
        };
        let job = inner
            .jobs
            .get_mut(&id)
            .ok_or(JobQueueError::Missing { id })?;
        job.attempts = job
            .attempts
            .checked_add(1)
            .ok_or_else(|| JobQueueError::Stored("attempt overflow".to_owned()))?;
        job.locked = true;
        let attempt =
            u32::try_from(job.attempts).map_err(|_| JobQueueError::Stored("attempt".to_owned()))?;
        let claimed = ClaimedJob {
            id,
            name: job.name.clone(),
            payload: job.payload.clone(),
            attempt,
        };
        drop(inner);
        Ok(Some(claimed))
    }

    async fn complete(&self, id: JobId) -> Result<(), JobQueueError> {
        let mut inner = self.inner.lock();
        if inner.jobs.remove(&id).is_none() {
            drop(inner);
            return Err(JobQueueError::Missing { id });
        }
        drop(inner);
        Ok(())
    }

    async fn release(&self, id: JobId) -> Result<(), JobQueueError> {
        let mut inner = self.inner.lock();
        let outcome = match inner.jobs.get_mut(&id) {
            Some(job) if job.locked => match job.attempts.checked_sub(1) {
                Some(attempts) => {
                    job.attempts = attempts;
                    job.locked = false;
                    Ok(())
                }
                None => Err(JobQueueError::Stored("attempt underflow".to_owned())),
            },
            Some(_) | None => Err(JobQueueError::Missing { id }),
        };
        drop(inner);
        outcome
    }

    async fn fail(&self, id: JobId, error: &str) -> Result<(), JobQueueError> {
        let mut inner = self.inner.lock();
        let job = inner
            .jobs
            .get_mut(&id)
            .ok_or(JobQueueError::Missing { id })?;
        job.last_error = Some(error.to_owned());
        job.locked = false;
        drop(inner);
        Ok(())
    }

    async fn revive_dead(
        &self,
        name: &JobName,
        dead_before: DateTime<Utc>,
    ) -> Result<u64, JobQueueError> {
        let mut inner = self.inner.lock();
        let mut revived = 0_u64;
        for job in inner.jobs.values_mut() {
            if job.name == *name && job.attempts >= job.max_attempts && job.run_at < dead_before {
                job.attempts = 0;
                job.locked = false;
                job.last_error = None;
                revived = revived.saturating_add(1);
            }
        }
        drop(inner);
        Ok(revived)
    }

    async fn read(&self, id: JobId) -> Result<Option<StoredJob>, JobQueueError> {
        let inner = self.inner.lock();
        let stored = match inner.jobs.get(&id) {
            Some(job) => {
                let attempts = u32::try_from(job.attempts)
                    .map_err(|_| JobQueueError::Stored("attempts".to_owned()))?;
                Some(StoredJob {
                    id: job.id,
                    name: job.name.clone(),
                    payload: job.payload.clone(),
                    key: job.key.clone(),
                    priority: Some(job.priority),
                    max_attempts: job.max_attempts,
                    attempts,
                    last_error: job.last_error.clone(),
                })
            }
            None => None,
        };
        drop(inner);
        Ok(stored)
    }
}

fn selectable(job: &MemJob, selector: &ClaimSelector) -> bool {
    if job.locked || job.attempts >= job.max_attempts {
        return false;
    }
    if let Some(only) = selector.only_id
        && job.id != only
    {
        return false;
    }
    selector.names.iter().any(|name| name == &job.name)
}

fn prefer(jobs: &BTreeMap<JobId, MemJob>, current: JobId, candidate: JobId) -> JobId {
    let Some(left) = jobs.get(&current) else {
        return candidate;
    };
    let Some(right) = jobs.get(&candidate) else {
        return current;
    };
    if right.priority < left.priority {
        return candidate;
    }
    if right.priority > left.priority {
        return current;
    }
    if right.run_at < left.run_at || (right.run_at == left.run_at && right.id < left.id) {
        candidate
    } else {
        current
    }
}
