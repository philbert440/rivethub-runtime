use kernel::tasks::Task;
use kernel::tasks::TaskFilter;
use kernel::tasks::TaskWatch;
use tokio::sync::broadcast;
use tokio::sync::watch;
use uuid::Uuid;

use super::TaskMemory;

impl TaskWatch for TaskMemory {
    fn subscribe(&self) -> watch::Receiver<u64> {
        self.generation.subscribe()
    }

    fn changed(&self) -> broadcast::Receiver<Uuid> {
        self.finished.subscribe()
    }
}

pub(super) fn listed(task: &Task, filter: &TaskFilter) -> bool {
    filter.status.is_none_or(|status| task.status == status)
        && filter
            .agent
            .as_ref()
            .is_none_or(|agent| task.target.agent == *agent)
}
