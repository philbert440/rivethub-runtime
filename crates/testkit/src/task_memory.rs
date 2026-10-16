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
use tokio::sync::watch;
use uuid::Uuid;
use wire::task::TaskPermissionDecision;
use wire::task::TaskStatus;

mod query;

use kernel::queue::FixedJob;
use kernel::tasks::ConflictAction;
use kernel::tasks::Finish;
use kernel::tasks::TaskError;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use kernel::tasks::terminal;

#[derive(Debug)]
struct MemoryInner {
    tasks: BTreeMap<Uuid, TaskRecord>,
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
    async fn insert(&self, task: &TaskRecord) -> Result<(), TaskError> {
        let mut inner = self.inner.lock();
        if inner.tasks.contains_key(&task.id) {
            drop(inner);
            return Err(TaskError::Stored("duplicate task".to_string()));
        }
        inner.tasks.insert(task.id, task.clone());
        drop(inner);
        self.touch();
        Ok(())
    }

    async fn load(&self, id: Uuid) -> Result<TaskRecord, TaskError> {
        self.inner
            .lock()
            .tasks
            .get(&id)
            .cloned()
            .ok_or(TaskError::Missing { id })
    }

    async fn mark_running(
        &self,
        id: Uuid,
        claimed_by: &str,
        node: &str,
        now: DateTime<Utc>,
    ) -> Result<TaskRecord, TaskError> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(TaskError::Missing { id })?;
        let claimable = matches!(task.status, TaskStatus::Queued | TaskStatus::AwaitingInput);
        let affinity = task
            .node_affinity
            .as_deref()
            .is_none_or(|affinity| affinity == node);
        if !claimable || !affinity {
            let status = task.status;
            drop(inner);
            return Err(TaskError::Conflict {
                id,
                status,
                action: ConflictAction::Run,
            });
        }
        let pending = task.pending_message.clone();
        task.status = TaskStatus::Running;
        task.attempt = task
            .attempt
            .checked_add(1)
            .ok_or(TaskError::Stored("attempt overflow".to_string()))?;
        task.started_at = Some(now);
        task.claimed_by = Some(claimed_by.to_string());
        task.last_heartbeat_at = Some(now);
        task.pending_message = None;
        let mut running = task.clone();
        running.pending_message = pending;
        drop(inner);
        self.touch();
        Ok(running)
    }

    async fn release_running(&self, id: Uuid) -> Result<(), TaskError> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(TaskError::Missing { id })?;
        if task.status != TaskStatus::Running {
            let status = task.status;
            drop(inner);
            return Err(TaskError::Conflict {
                id,
                status,
                action: ConflictAction::Release,
            });
        }
        task.attempt = task
            .attempt
            .checked_sub(1)
            .ok_or_else(|| TaskError::Stored("attempt underflow".to_owned()))?;
        task.status = TaskStatus::Queued;
        task.claimed_by = None;
        task.started_at = None;
        drop(inner);
        self.touch();
        Ok(())
    }

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, TaskError> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(TaskError::Missing { id })?;
        if task.status == TaskStatus::Killed {
            drop(inner);
            return Ok(false);
        }
        task.status = finish.status;
        task.result.clone_from(&finish.result);
        task.error.clone_from(&finish.error);
        task.usage = finish.usage;
        task.completed_at = Some(finish.completed_at);
        task.duration_ms = finish.duration_ms;
        task.pending_message = None;
        drop(inner);
        self.touch();
        Ok(true)
    }

    async fn sweep(&self, worker: &str, now: DateTime<Utc>) -> Result<(), TaskError> {
        let stale_before = now
            .checked_sub_signed(chrono::Duration::milliseconds(90_000))
            .ok_or(TaskError::Clock)?;
        let mut inner = self.inner.lock();
        let ids: Vec<Uuid> = inner.tasks.keys().copied().collect();
        for id in ids {
            let task = inner.tasks.get_mut(&id).ok_or(TaskError::Missing { id })?;
            if task.status == TaskStatus::Running && task.claimed_by.as_deref() == Some(worker) {
                let stamp = task.last_heartbeat_at.or(task.started_at);
                if let Some(stamp) = stamp
                    && stamp < stale_before
                {
                    if task.attempt < task.max_attempts {
                        task.status = TaskStatus::Queued;
                        task.claimed_by = None;
                        task.completed_at = None;
                    } else {
                        task.status = TaskStatus::Failed;
                        task.error = Some("worker_restarted".to_string());
                        task.completed_at = Some(now);
                    }
                }
            }
            if task.status == TaskStatus::AwaitingInput {
                let ttl_ms = task.budget.max_wall_clock_ms.unwrap_or(86_400_000);
                let ttl_ms = i64::try_from(ttl_ms).map_err(|_| TaskError::Clock)?;
                let deadline = now
                    .checked_sub_signed(chrono::Duration::milliseconds(ttl_ms))
                    .ok_or(TaskError::Clock)?;
                let stamp = match task.last_heartbeat_at.or(task.started_at) {
                    Some(stamp) => stamp,
                    None => task.created_at,
                };
                if stamp < deadline {
                    task.status = TaskStatus::Timeout;
                    task.error = Some("awaiting-input expired".to_string());
                    task.completed_at = Some(now);
                }
            }
        }
        drop(inner);
        self.touch();
        Ok(())
    }

    async fn kill_queued_by_session(
        &self,
        session_key: &str,
        now: DateTime<Utc>,
    ) -> Result<(), TaskError> {
        let mut inner = self.inner.lock();
        for task in inner.tasks.values_mut() {
            if task.session_key.as_deref() == Some(session_key) && task.status == TaskStatus::Queued
            {
                task.status = TaskStatus::Killed;
                task.completed_at = Some(now);
                task.error = Some("replaced by a newer heartbeat".to_string());
            }
        }
        drop(inner);
        self.touch();
        Ok(())
    }

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, TaskError> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(TaskError::Missing { id })?;
        let prior = task.status;
        if terminal(prior) {
            drop(inner);
            return Err(TaskError::Conflict {
                id,
                status: prior,
                action: ConflictAction::Kill,
            });
        }
        task.status = TaskStatus::Killed;
        task.completed_at = Some(now);
        if task.error.is_none() {
            task.error = Some("Killed by parent".to_string());
        }
        drop(inner);
        self.touch();
        Ok(prior)
    }

    async fn set_pending(&self, id: Uuid, message: &str) -> Result<(), TaskError> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(TaskError::Missing { id })?;
        let open = matches!(
            task.status,
            TaskStatus::Queued | TaskStatus::Running | TaskStatus::AwaitingInput
        );
        if !open {
            let status = task.status;
            drop(inner);
            return Err(TaskError::Conflict {
                id,
                status,
                action: ConflictAction::Steer,
            });
        }
        task.pending_message = Some(message.to_string());
        drop(inner);
        self.touch();
        Ok(())
    }

    async fn add_harness_session(&self, id: Uuid, session: &str) -> Result<(), TaskError> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(TaskError::Missing { id })?;
        task.harness_session_ids.push(session.to_string());
        drop(inner);
        self.touch();
        Ok(())
    }

    async fn append_permission(
        &self,
        id: Uuid,
        decision: &TaskPermissionDecision,
    ) -> Result<(), TaskError> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(TaskError::Missing { id })?;
        let decisions = task.spec.permission_decisions.get_or_insert_with(Vec::new);
        decisions.push(decision.clone());
        drop(inner);
        self.touch();
        Ok(())
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
            WorkerRole::Categorizer => vec![JobName::Fixed(FixedJob::SuggestTags)],
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
