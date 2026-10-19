use std::sync::atomic::Ordering;

use async_trait::async_trait;
use kernel::tasks::TaskError;
use kernel::tasks::TaskListFilter;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskWatch;
use tokio::sync::broadcast;
use tokio::sync::watch;
use uuid::Uuid;

use super::TaskMemory;

#[async_trait]
impl TaskQuery for TaskMemory {
    async fn list(&self, filter: &TaskListFilter) -> Result<Vec<TaskRecord>, TaskError> {
        if self.fail_list.swap(false, Ordering::SeqCst) {
            return Err(TaskError::QueueAbsent);
        }
        let mut tasks = {
            let inner = self.inner.lock();
            inner.tasks.values().cloned().collect::<Vec<_>>()
        };
        tasks.retain(|task| listed(task, filter));
        tasks.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then(right.id.cmp(&left.id))
        });
        if let Some(limit) = filter.limit {
            let limit =
                usize::try_from(limit).map_err(|_| TaskError::Stored("limit".to_string()))?;
            if tasks.len() > limit {
                tasks.truncate(limit);
            }
        }
        Ok(tasks)
    }
}

impl TaskWatch for TaskMemory {
    fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }

    fn finished(&self) -> broadcast::Receiver<Uuid> {
        self.finished.subscribe()
    }
}

fn listed(task: &TaskRecord, filter: &TaskListFilter) -> bool {
    if let Some(status) = filter.status
        && task.status != status
    {
        return false;
    }
    if let Some(agent) = &filter.agent_id
        && task.agent_id != *agent
    {
        return false;
    }
    true
}
