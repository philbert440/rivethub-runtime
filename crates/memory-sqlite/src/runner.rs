use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use parking_lot::Mutex;
use thiserror::Error;

use crate::SqliteStore;
use crate::error::SqliteError;
use crate::queue::ClaimedSqliteJob;

#[derive(Debug, Error)]
#[error("{message}")]
pub struct JobFailure {
    message: String,
}

impl JobFailure {
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[async_trait]
pub trait JobHandler: Send + Sync {
    async fn run(&self, job: &ClaimedSqliteJob) -> Result<(), JobFailure>;
}

#[async_trait]
pub trait JobSweep: Send + Sync {
    async fn run(&self, now: DateTime<Utc>) -> Result<(), JobFailure>;
}

struct RegisteredSweep {
    name: String,
    every: chrono::Duration,
    sweep: Arc<dyn JobSweep>,
}

struct ClearRunning<'a>(&'a AtomicBool);

impl Drop for ClearRunning<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

pub struct JobRunner {
    store: SqliteStore,
    handlers: BTreeMap<String, Arc<dyn JobHandler>>,
    sweeps: Vec<RegisteredSweep>,
    last_sweep: Mutex<BTreeMap<String, DateTime<Utc>>>,
    running: AtomicBool,
    halted: AtomicBool,
    recovered: AtomicBool,
    sweeps_completed: AtomicU64,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "derive(Debug) cannot format the handler and sweep trait objects"
    )
)]
impl std::fmt::Debug for JobRunner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("JobRunner")
            .field("store", &self.store)
            .field("handlers", &self.handlers.keys().collect::<Vec<_>>())
            .field(
                "sweeps",
                &self
                    .sweeps
                    .iter()
                    .map(|sweep| sweep.name.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("last_sweep", &self.last_sweep)
            .field("running", &self.is_running())
            .field("halted", &self.halted.load(Ordering::Acquire))
            .field("recovered", &self.recovered.load(Ordering::Acquire))
            .field("sweeps_completed", &self.sweeps_completed())
            .finish()
    }
}

impl JobRunner {
    #[must_use]
    pub fn new(store: SqliteStore) -> Self {
        Self {
            store,
            handlers: BTreeMap::new(),
            sweeps: Vec::new(),
            last_sweep: Mutex::new(BTreeMap::new()),
            running: AtomicBool::new(false),
            halted: AtomicBool::new(false),
            recovered: AtomicBool::new(false),
            sweeps_completed: AtomicU64::new(0),
        }
    }

    pub fn on_task(&mut self, task: impl Into<String>, handler: Arc<dyn JobHandler>) {
        self.handlers.insert(task.into(), handler);
    }

    pub fn on_sweep(
        &mut self,
        name: impl Into<String>,
        every: chrono::Duration,
        sweep: Arc<dyn JobSweep>,
    ) {
        self.sweeps.push(RegisteredSweep {
            name: name.into(),
            every,
            sweep,
        });
    }

    pub fn halt(&self) {
        self.halted.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    #[must_use]
    pub fn sweeps_completed(&self) -> u64 {
        self.sweeps_completed.load(Ordering::Acquire)
    }

    pub async fn tick(&self, now: DateTime<Utc>, batch: u32) -> Result<u64, SqliteError> {
        if self.halted.load(Ordering::Acquire) {
            return Ok(0);
        }
        self.running.store(true, Ordering::Release);
        let _guard = ClearRunning(&self.running);
        self.recover_once(now).await?;
        self.run_sweeps(now).await;
        self.run_batch(now, batch).await
    }

    async fn recover_once(&self, now: DateTime<Utc>) -> Result<(), SqliteError> {
        let first = self
            .recovered
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if first {
            self.store.recover_running(now).await?;
        }
        Ok(())
    }

    async fn run_sweeps(&self, now: DateTime<Utc>) {
        for sweep in &self.sweeps {
            if !self.sweep_due(&sweep.name, sweep.every, now) {
                continue;
            }
            let outcome = sweep.sweep.run(now).await;
            self.last_sweep.lock().insert(sweep.name.clone(), now);
            match outcome {
                Ok(()) => {
                    self.sweeps_completed.fetch_add(1, Ordering::AcqRel);
                }
                Err(error) => self.store.record_log(format!(
                    "[sqlite-jobs] sweep {} failed: {error}",
                    sweep.name
                )),
            }
        }
    }

    fn sweep_due(&self, name: &str, every: chrono::Duration, now: DateTime<Utc>) -> bool {
        match self.last_sweep.lock().get(name) {
            None => true,
            Some(last) => now.signed_duration_since(*last) >= every,
        }
    }

    async fn run_batch(&self, now: DateTime<Utc>, batch: u32) -> Result<u64, SqliteError> {
        let tasks: Vec<String> = self.handlers.keys().cloned().collect();
        let mut completed: u64 = 0;
        for _ in 0..batch {
            if self.halted.load(Ordering::Acquire) {
                break;
            }
            let Some(job) = self.store.claim_tasks(tasks.clone(), now).await? else {
                break;
            };
            let worker = match self.handlers.get(&job.task) {
                Some(registered) => Arc::clone(registered),
                None => {
                    return Err(SqliteError::NoHandler {
                        task: job.task.clone(),
                    });
                }
            };
            self.finish_job(&worker, &job, now).await?;
            completed += 1;
        }
        Ok(completed)
    }

    async fn finish_job(
        &self,
        handler: &Arc<dyn JobHandler>,
        job: &ClaimedSqliteJob,
        now: DateTime<Utc>,
    ) -> Result<(), SqliteError> {
        match handler.run(job).await {
            Ok(()) => self.store.complete_job(job.id.clone()).await,
            Err(error) => self.fail_and_log(job, &error, now).await,
        }
    }

    async fn fail_and_log(
        &self,
        job: &ClaimedSqliteJob,
        error: &JobFailure,
        now: DateTime<Utc>,
    ) -> Result<(), SqliteError> {
        let outcome = self
            .store
            .fail_job(job.id.clone(), error.to_string(), now)
            .await?;
        let key = job.key.as_deref().unwrap_or_default();
        self.store.record_log(format!(
            "[sqlite-jobs] {} {key} failed (attempt {}/{}, {outcome}): {error}",
            job.task, job.attempts, job.max_attempts
        ));
        Ok(())
    }
}
