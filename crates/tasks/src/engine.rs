use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::Notify;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

use kernel::queue::JobQueue;
use kernel::shutdown::Activities;
use kernel::tasks::TaskStore;

use crate::directory::Directory;
use crate::error::Error;
use crate::price::TokenRate;
use crate::price::table;

pub struct Settings {
    pub node: String,
    pub worker: String,
    pub files_root: Option<PathBuf>,
    pub pricing: BTreeMap<String, TokenRate>,
    pub api_token: Option<String>,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "Debug names the worker and whether an api token is set; it must not print the token"
    )
)]
impl std::fmt::Debug for Settings {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Settings")
            .field("node", &self.node)
            .field("worker", &self.worker)
            .field("files_root", &self.files_root)
            .field("pricing_models", &self.pricing.len())
            .field("has_api_token", &self.api_token.is_some())
            .finish()
    }
}

#[derive(Debug)]
pub struct Engine<S, J> {
    pub(crate) store: S,
    pub(crate) jobs: J,
    pub(crate) directory: Directory,
    pub(crate) settings: Settings,
    wake: Arc<Notify>,
    finished: watch::Sender<Option<Uuid>>,
    pub(crate) tracker: TaskTracker,
    inflight: Arc<Mutex<HashMap<Uuid, CancellationToken>>>,
    pub(crate) activities: Activities,
}

impl<S, J> Engine<S, J>
where
    S: TaskStore,
    J: JobQueue,
{
    pub fn new(
        store: S,
        jobs: J,
        directory: Directory,
        mut settings: Settings,
    ) -> Result<Self, Error> {
        if settings.node.is_empty() || settings.worker.is_empty() {
            return Err(Error::NodeRequired);
        }
        settings.pricing = table(std::mem::take(&mut settings.pricing))?;
        let (finished, _receiver) = watch::channel(None);
        Ok(Self {
            store,
            jobs,
            directory,
            settings,
            wake: Arc::new(Notify::new()),
            finished,
            tracker: TaskTracker::new(),
            inflight: Arc::new(Mutex::new(HashMap::new())),
            activities: Activities::new(),
        })
    }

    #[must_use]
    pub fn with_activities(mut self, activities: Activities) -> Self {
        self.activities = activities;
        self
    }

    pub fn subscribe(&self) -> watch::Receiver<Option<Uuid>> {
        self.finished.subscribe()
    }

    pub fn wake_waiters(&self) {
        self.finished.send_modify(|_finished| {});
    }

    pub(crate) fn publish(&self, id: Uuid) {
        let _previous = self.finished.send_replace(Some(id));
    }

    pub(crate) fn notify_jobs(&self) {
        self.wake.notify_one();
    }

    pub(crate) fn wake(&self) -> Arc<Notify> {
        Arc::clone(&self.wake)
    }

    pub(crate) fn begin_run(&self, id: Uuid, parent: &CancellationToken) -> CancellationToken {
        let child = parent.child_token();
        self.inflight.lock().insert(id, child.clone());
        child
    }

    pub(crate) fn end_run(&self, id: Uuid) {
        drop(self.inflight.lock().remove(&id));
    }

    pub(crate) fn cancel_run(&self, id: Uuid) {
        if let Some(token) = self.inflight.lock().get(&id) {
            token.cancel();
        }
    }
}
