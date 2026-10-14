use async_trait::async_trait;
use kernel::queue::ClaimedJob;
use kernel::queue::Job;
use kernel::queue::JobId;
use kernel::queue::JobQueue;
use kernel::queue::JobQueueError;
use kernel::queue::WorkerRole;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;
use tokio::sync::Mutex;

#[derive(Debug, Default)]
pub struct FakeJobQueue {
    jobs: Mutex<BTreeMap<JobId, Job>>,
    next_id: AtomicI64,
}

#[async_trait]
impl JobQueue for FakeJobQueue {
    async fn enqueue(&self, job: Job) -> Result<(), JobQueueError> {
        let id = JobId::new(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.jobs.lock().await.insert(id, job);
        Ok(())
    }

    async fn claim(&self, _role: WorkerRole) -> Result<Option<ClaimedJob>, JobQueueError> {
        let claimed = {
            let mut jobs = self.jobs.lock().await;
            jobs.iter()
                .next()
                .map(|(id, job)| (*id, job.clone()))
                .map(|(id, job)| {
                    jobs.remove(&id);
                    ClaimedJob {
                        id,
                        name: job.name,
                        payload: job.payload,
                        attempt: 1,
                    }
                })
        };
        Ok(claimed)
    }

    async fn complete(&self, id: JobId) -> Result<(), JobQueueError> {
        self.jobs.lock().await.remove(&id);
        Ok(())
    }

    async fn fail(&self, _id: JobId, _error: &str) -> Result<(), JobQueueError> {
        Ok(())
    }

    async fn revive_dead(
        &self,
        _name: &kernel::queue::JobName,
        _dead_before: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, JobQueueError> {
        Ok(0)
    }
}
