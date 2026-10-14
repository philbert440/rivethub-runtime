use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::queue::JobKeyMode;
use parking_lot::Mutex;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::task::TaskStatus;

use crate::error::ConflictAction;
use crate::error::Error;
use crate::jobs::ClaimSelector;
use crate::jobs::ClaimedJob;
use crate::jobs::JobWake;
use crate::jobs::NewJob;
use crate::jobs::StoredJob;
use crate::jobs::TaskJobs;
use crate::record::Finish;
use crate::record::TaskRecord;
use crate::record::terminal;
use crate::store::TaskStore;

#[derive(Debug)]
struct MemoryInner {
    tasks: BTreeMap<Uuid, TaskRecord>,
    jobs: BTreeMap<i64, MemJob>,
    next_job: i64,
}

#[derive(Debug, Clone)]
struct MemJob {
    id: i64,
    name: String,
    payload_text: String,
    payload: crate::payload::TaskJobPayload,
    key: String,
    priority: i16,
    max_attempts: i32,
    attempts: i32,
    locked: bool,
    run_at: DateTime<Utc>,
    last_error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Memory {
    inner: Arc<Mutex<MemoryInner>>,
    fail_enqueue: Arc<AtomicBool>,
}

impl Memory {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(MemoryInner {
                tasks: BTreeMap::new(),
                jobs: BTreeMap::new(),
                next_job: 1,
            })),
            fail_enqueue: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn fail_next_enqueue(&self) {
        self.fail_enqueue.store(true, Ordering::SeqCst);
    }
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "the next job id starts at 1; Default would zero it and the first enqueue would use id 0"
    )
)]
impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl TaskStore for Memory {
    async fn insert(&self, task: &TaskRecord) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        if inner.tasks.contains_key(&task.id) {
            drop(inner);
            return Err(Error::Stored("duplicate task".to_string()));
        }
        inner.tasks.insert(task.id, task.clone());
        drop(inner);
        Ok(())
    }

    async fn load(&self, id: Uuid) -> Result<TaskRecord, Error> {
        self.inner
            .lock()
            .tasks
            .get(&id)
            .cloned()
            .ok_or(Error::Missing { id })
    }

    async fn mark_running(
        &self,
        id: Uuid,
        claimed_by: &str,
        node: &str,
        now: DateTime<Utc>,
    ) -> Result<TaskRecord, Error> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(Error::Missing { id })?;
        let claimable = matches!(task.status, TaskStatus::Queued | TaskStatus::AwaitingInput);
        let affinity = task
            .node_affinity
            .as_deref()
            .is_none_or(|affinity| affinity == node);
        if !claimable || !affinity {
            let status = task.status;
            drop(inner);
            return Err(Error::Conflict {
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
            .ok_or(Error::Stored("attempt overflow".to_string()))?;
        task.started_at = Some(now);
        task.claimed_by = Some(claimed_by.to_string());
        task.last_heartbeat_at = Some(now);
        task.pending_message = None;
        let mut running = task.clone();
        running.pending_message = pending;
        drop(inner);
        Ok(running)
    }

    async fn finish(&self, id: Uuid, finish: &Finish) -> Result<bool, Error> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(Error::Missing { id })?;
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
        Ok(true)
    }

    async fn sweep(&self, worker: &str, now: DateTime<Utc>) -> Result<(), Error> {
        let stale_before = now
            .checked_sub_signed(chrono::Duration::milliseconds(90_000))
            .ok_or(Error::Clock)?;
        let mut inner = self.inner.lock();
        let ids: Vec<Uuid> = inner.tasks.keys().copied().collect();
        for id in ids {
            let task = inner.tasks.get_mut(&id).ok_or(Error::Missing { id })?;
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
                #[expect(
                    clippy::manual_unwrap_or,
                    reason = "core-domain.md:399 awaiting-input TTL is maxWallClockMs else 24h; unwrap_or is rejected as a silent fallback"
                )]
                let ttl_ms = match task.budget.max_wall_clock_ms {
                    Some(ms) => ms,
                    None => 86_400_000,
                };
                let ttl_ms = i64::try_from(ttl_ms).map_err(|_| Error::Clock)?;
                let deadline = now
                    .checked_sub_signed(chrono::Duration::milliseconds(ttl_ms))
                    .ok_or(Error::Clock)?;
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
        Ok(())
    }

    async fn kill_queued_by_session(
        &self,
        session_key: &str,
        now: DateTime<Utc>,
    ) -> Result<(), Error> {
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
        Ok(())
    }

    async fn kill(&self, id: Uuid, now: DateTime<Utc>) -> Result<TaskStatus, Error> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(Error::Missing { id })?;
        let prior = task.status;
        if terminal(prior) {
            drop(inner);
            return Err(Error::Conflict {
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
        Ok(prior)
    }

    async fn set_pending(&self, id: Uuid, message: &str) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(Error::Missing { id })?;
        let open = matches!(
            task.status,
            TaskStatus::Queued | TaskStatus::Running | TaskStatus::AwaitingInput
        );
        if !open {
            let status = task.status;
            drop(inner);
            return Err(Error::Conflict {
                id,
                status,
                action: ConflictAction::Steer,
            });
        }
        task.pending_message = Some(message.to_string());
        drop(inner);
        Ok(())
    }

    async fn add_harness_session(&self, id: Uuid, session: &str) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        let task = inner.tasks.get_mut(&id).ok_or(Error::Missing { id })?;
        task.harness_session_ids.push(session.to_string());
        drop(inner);
        Ok(())
    }
}

#[async_trait]
impl TaskJobs for Memory {
    async fn enqueue(&self, job: &NewJob) -> Result<i64, Error> {
        if self.fail_enqueue.swap(false, Ordering::SeqCst) {
            return Err(Error::QueueAbsent);
        }
        let payload_text = serde_json::to_string(&job.payload)
            .map_err(|source| Error::Payload(Box::new(source)))?;
        let mut inner = self.inner.lock();
        if let Some(existing) = inner.jobs.values_mut().find(|item| item.key == job.key) {
            existing.name = job.name.to_string();
            existing.payload_text = payload_text;
            existing.payload = job.payload.clone();
            existing.priority = job.priority;
            existing.max_attempts = job.max_attempts;
            if !existing.locked && job.key_mode == JobKeyMode::Replace {
                existing.attempts = 0;
                existing.last_error = None;
            }
            let id = existing.id;
            drop(inner);
            return Ok(id);
        }
        let id = inner.next_job;
        inner.next_job = inner
            .next_job
            .checked_add(1)
            .ok_or(Error::Stored("job id overflow".to_string()))?;
        inner.jobs.insert(
            id,
            MemJob {
                id,
                name: job.name.to_string(),
                payload_text,
                payload: job.payload.clone(),
                key: job.key.clone(),
                priority: job.priority,
                max_attempts: job.max_attempts,
                attempts: 0,
                locked: false,
                run_at: Utc::now(),
                last_error: None,
            },
        );
        drop(inner);
        Ok(id)
    }

    async fn claim(&self, selector: &ClaimSelector) -> Result<Option<ClaimedJob>, Error> {
        let mut inner = self.inner.lock();
        let mut choice: Option<i64> = None;
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
        let job = inner.jobs.get_mut(&id).ok_or(Error::MissingJob { id })?;
        job.attempts = job
            .attempts
            .checked_add(1)
            .ok_or(Error::Stored("attempt overflow".to_string()))?;
        job.locked = true;
        let attempt =
            u32::try_from(job.attempts).map_err(|_| Error::Stored("attempt".to_string()))?;
        let claimed = ClaimedJob {
            id,
            identifier: job.name.clone(),
            payload_text: job.payload_text.clone(),
            attempt,
        };
        drop(inner);
        Ok(Some(claimed))
    }

    async fn complete(&self, id: i64) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        if inner.jobs.remove(&id).is_none() {
            drop(inner);
            return Err(Error::MissingJob { id });
        }
        drop(inner);
        Ok(())
    }

    async fn fail(&self, id: i64, error: &str) -> Result<(), Error> {
        let mut inner = self.inner.lock();
        let job = inner.jobs.get_mut(&id).ok_or(Error::MissingJob { id })?;
        job.last_error = Some(error.to_string());
        job.locked = false;
        drop(inner);
        Ok(())
    }

    async fn read(&self, id: i64) -> Result<Option<StoredJob>, Error> {
        let inner = self.inner.lock();
        let stored = match inner.jobs.get(&id) {
            Some(job) => {
                let attempts = u32::try_from(job.attempts)
                    .map_err(|_| Error::Stored("attempts".to_string()))?;
                Some(StoredJob {
                    id: job.id,
                    identifier: job.name.clone(),
                    payload: job.payload.clone(),
                    key: Some(job.key.clone()),
                    priority: job.priority,
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

    async fn wake(&self, cancel: CancellationToken) -> Result<JobWake, Error> {
        Ok(JobWake::Memory { cancel })
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

fn prefer(jobs: &BTreeMap<i64, MemJob>, current: i64, candidate: i64) -> i64 {
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
