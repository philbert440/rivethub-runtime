use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use kernel::tasks::Task;
use parking_lot::Mutex;
use tokio::time::Instant;
use uuid::Uuid;

use super::Tasks;
use crate::Gateway;

const GRACE: Duration = Duration::from_secs(3);

#[derive(Debug, Default)]
pub(crate) struct Waits {
    inner: Mutex<BTreeMap<Uuid, Wait>>,
}

#[derive(Debug, Clone, Copy)]
struct Wait {
    active: usize,
    since: Instant,
    claimed: bool,
    delivering: bool,
}

impl Wait {
    fn new() -> Self {
        Self {
            active: 0,
            since: Instant::now(),
            claimed: false,
            delivering: false,
        }
    }
}

enum Idle {
    Taken,
    Since(Instant),
}

pub(crate) struct WaitGuard<'a> {
    waits: &'a Waits,
    id: Uuid,
}

impl Drop for WaitGuard<'_> {
    fn drop(&mut self) {
        if let Some(wait) = self.waits.inner.lock().get_mut(&self.id) {
            wait.active = wait.active.saturating_sub(1);
            wait.since = Instant::now();
        }
    }
}

impl Waits {
    pub(crate) fn created(&self, id: Uuid) {
        self.inner.lock().insert(id, Wait::new());
    }

    pub(crate) fn enter(&self, id: Uuid) -> WaitGuard<'_> {
        self.inner.lock().entry(id).or_insert_with(Wait::new).active += 1;
        WaitGuard { waits: self, id }
    }

    pub(crate) fn claim(&self, id: Uuid) {
        if let Some(wait) = self.inner.lock().get_mut(&id) {
            wait.claimed = true;
        }
    }

    pub(super) fn deliver(&self, id: Uuid) -> bool {
        match self.inner.lock().get_mut(&id) {
            Some(wait) if !wait.claimed && !wait.delivering => {
                wait.delivering = true;
                true
            }
            Some(_) | None => false,
        }
    }

    fn idle(&self, id: Uuid) -> Idle {
        match self.inner.lock().get(&id) {
            Some(wait) if !wait.claimed && wait.active == 0 => Idle::Since(wait.since),
            Some(_) | None => Idle::Taken,
        }
    }

    pub(super) fn forget(&self, id: Uuid) {
        self.inner.lock().remove(&id);
    }
}

impl Tasks {
    pub(super) async fn deliver_late(self: Arc<Self>, gateway: Arc<Gateway>, task: Task) {
        let Some(parent) = task.parent.session.clone() else {
            return;
        };
        loop {
            match self.waits.idle(task.id) {
                Idle::Taken => {
                    self.waits.forget(task.id);
                    return;
                }
                Idle::Since(since) if since + GRACE <= Instant::now() => break,
                Idle::Since(since) => tokio::time::sleep_until(since + GRACE).await,
            }
        }
        self.waits.forget(task.id);
        let Some(chat) = gateway
            .terminal()
            .and_then(|terminal| terminal.live_chat_of(&parent))
        else {
            return;
        };
        if let Err(error) = chat
            .prompt(late_text(&task), None, Vec::new(), Vec::new())
            .await
        {
            tracing::warn!(%error, task = %task.id, "a late task result was not delivered");
        }
    }
}

fn late_text(task: &Task) -> String {
    let said = task
        .result
        .as_ref()
        .map(|result| result.summary.trim())
        .filter(|summary| !summary.is_empty())
        .or(task.error.as_deref())
        .unwrap_or("(no reply)");
    let child = task.child.as_deref().unwrap_or("no session");
    format!(
        "Task {} finished ({}) after your delegate_task call stopped waiting.\n\n{said}\n\nChild session {child} on {}.",
        task.id, task.status, task.target.node
    )
}
