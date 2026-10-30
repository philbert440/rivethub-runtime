mod child;
mod directory;
mod late;
mod paths;
mod runner;
mod submit;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use kernel::queue::JobQueue;
use kernel::tasks::TaskStore;
use kernel::tasks::TaskWatch;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

pub use directory::LocalAgent;
pub(crate) use directory::Refusal;
pub(crate) use late::Waits;
pub use runner::serve;
pub(crate) use submit::Submission;
pub(crate) use submit::SubmitError;

use crate::Gateway;
use crate::mesh::Registry;
use kernel::tasks::Harness;
use wire::harness::HarnessId;

#[expect(
    missing_debug_implementations,
    reason = "kernel ports intentionally do not require Debug"
)]
pub struct Tasks {
    pub(crate) store: Arc<dyn TaskStore>,
    pub(crate) jobs: Arc<dyn JobQueue>,
    pub(crate) watch: Arc<dyn TaskWatch>,
    pub(crate) node: String,
    agents: BTreeMap<String, LocalAgent>,
    mesh: Option<Registry>,
    files_root: Option<PathBuf>,
    pub(crate) waits: Waits,
    running: Mutex<BTreeMap<Uuid, CancellationToken>>,
}

impl Tasks {
    #[must_use]
    pub fn new(
        store: Arc<dyn TaskStore>,
        jobs: Arc<dyn JobQueue>,
        watch: Arc<dyn TaskWatch>,
        node: String,
    ) -> Self {
        Self {
            store,
            jobs,
            watch,
            node,
            agents: BTreeMap::new(),
            mesh: None,
            files_root: None,
            waits: Waits::default(),
            running: Mutex::new(BTreeMap::new()),
        }
    }

    #[must_use]
    pub fn with_agents(mut self, agents: BTreeMap<String, LocalAgent>) -> Self {
        self.agents = agents;
        self
    }

    #[must_use]
    pub fn with_mesh(mut self, registry: Registry) -> Self {
        self.mesh = Some(registry);
        self
    }

    #[must_use]
    pub fn with_files_root(mut self, root: Option<PathBuf>) -> Self {
        self.files_root = root;
        self
    }

    pub(crate) async fn sources(
        &self,
        gateway: &Gateway,
    ) -> Result<directory::Sources<'_>, SubmitError> {
        let presets = gateway
            .presets()
            .list(None)
            .await
            .map_err(|error| SubmitError::Unavailable(error.to_string()))?;
        let nodes = match &self.mesh {
            Some(registry) => Some(
                registry
                    .snapshot()
                    .await
                    .map_err(|error| SubmitError::Unavailable(error.to_string()))?
                    .map(|file| file.nodes.into_values().collect())
                    .unwrap_or_default(),
            ),
            None => None,
        };
        let harnessed = self
            .agents
            .keys()
            .filter_map(|name| {
                let delegate = gateway.terminal()?.delegate(name)?;
                Some((name.clone(), delegate.harness))
            })
            .collect();
        Ok(directory::Sources {
            node: &self.node,
            presets,
            local: &self.agents,
            harnessed,
            nodes,
        })
    }

    fn track(&self, id: Uuid, stop: &CancellationToken) {
        self.running.lock().insert(id, stop.clone());
    }

    fn untrack(&self, id: Uuid) {
        drop(self.running.lock().remove(&id));
    }

    fn running_ids(&self) -> Vec<Uuid> {
        self.running.lock().keys().copied().collect()
    }

    fn stop(&self, id: Uuid) {
        if let Some(stop) = self.running.lock().get(&id) {
            stop.cancel();
        }
    }
}

#[must_use]
pub const fn harness_in(harness: HarnessId) -> Harness {
    match harness {
        HarnessId::ClaudeCode => Harness::ClaudeCode,
        HarnessId::GrokBuild => Harness::GrokBuild,
        HarnessId::KimiCode => Harness::KimiCode,
        HarnessId::Hermes => Harness::Hermes,
        HarnessId::Codex => Harness::Codex,
        HarnessId::Opencode => Harness::Opencode,
        HarnessId::Omp => Harness::Omp,
        HarnessId::Dsh => Harness::Dsh,
        HarnessId::Cursor => Harness::Cursor,
        HarnessId::Cowork => Harness::Cowork,
    }
}

#[must_use]
pub const fn harness_out(harness: Harness) -> HarnessId {
    match harness {
        Harness::ClaudeCode => HarnessId::ClaudeCode,
        Harness::GrokBuild => HarnessId::GrokBuild,
        Harness::KimiCode => HarnessId::KimiCode,
        Harness::Hermes => HarnessId::Hermes,
        Harness::Codex => HarnessId::Codex,
        Harness::Opencode => HarnessId::Opencode,
        Harness::Omp => HarnessId::Omp,
        Harness::Dsh => HarnessId::Dsh,
        Harness::Cursor => HarnessId::Cursor,
        Harness::Cowork => HarnessId::Cowork,
    }
}
