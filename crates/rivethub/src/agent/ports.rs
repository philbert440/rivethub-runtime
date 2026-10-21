use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use kernel::queue::JobQueue;
use kernel::tasks::CatalogSource;
use kernel::tasks::TaskAgents;
use kernel::tasks::TaskError;
use kernel::tasks::TaskPresets;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskStore;
use kernel::tasks::TaskWatch;
use wire::gateway::agents::AgentPreset;
use wire::harness::HarnessId;
use wire::mesh::MeshNode;
use wire::mesh::MeshNodeStatus;

use super::LocalBinding;
use super::assemble::Plan;

#[derive(Debug)]
pub(super) struct PresetFile {
    state_dir: PathBuf,
}

impl PresetFile {
    pub(super) const fn new(state_dir: PathBuf) -> Self {
        Self { state_dir }
    }
}

#[async_trait]
impl TaskPresets for PresetFile {
    async fn presets(&self) -> Result<Vec<AgentPreset>, TaskError> {
        super::assemble::load_presets(self.state_dir.clone())
            .await
            .map_err(|error| TaskError::Operation {
                operation: "agent preset read",
                source: Box::new(error),
            })
    }
}

#[derive(Debug)]
pub(super) struct Roster {
    pub node: String,
    pub mesh: bool,
    pub local: BTreeMap<String, LocalBinding>,
    pub presets: Arc<PresetFile>,
    pub nodes: Vec<MeshNode>,
}

impl Roster {
    pub(super) fn directory(&self, plan: &Plan) -> Result<tasks::Directory, tasks::Error> {
        let mut directory = tasks::Directory::new(plan.node.clone(), plan.gateway.clone())?;
        for (name, binding) in &self.local {
            directory.local_agent(
                name.clone(),
                tasks::LocalAgent {
                    model: binding.model.clone(),
                    provider: binding.provider.clone(),
                    effort: None,
                    harness: None,
                    system_prompt: None,
                    working_dir: None,
                },
            );
        }
        for node in &self.nodes {
            directory.mesh_node(node.clone());
        }
        directory.set_mesh_identity(plan.mesh_identity.clone());
        directory.set_gateway_trust(plan.gateway_trust.clone());
        Ok(directory)
    }

    pub(super) fn presets(&self) -> Arc<dyn TaskPresets> {
        Arc::clone(&self.presets) as Arc<dyn TaskPresets>
    }

    async fn known(&self, agent_id: &str) -> Result<bool, TaskError> {
        if self.local.contains_key(agent_id) {
            return Ok(true);
        }
        if self
            .presets
            .presets()
            .await?
            .iter()
            .any(|preset| preset.name == agent_id || preset.id == agent_id)
        {
            return Ok(true);
        }
        Ok(self.nodes.iter().any(|node| {
            node.status == MeshNodeStatus::Online
                && node.agents.iter().any(|agent| agent == agent_id)
        }))
    }
}

#[async_trait]
impl TaskAgents for Roster {
    async fn contains(&self, agent_id: &str) -> Result<bool, TaskError> {
        self.known(agent_id).await
    }
}

pub(super) fn api<T>(
    roster: Arc<Roster>,
    catalog: Arc<tasks::Catalog>,
    allowed: Option<Vec<HarnessId>>,
    tasks: &T,
) -> Arc<gateway::routes::tasks::TasksApi>
where
    T: TaskStore + JobQueue + TaskQuery + TaskWatch + Clone + 'static,
{
    let store: Arc<dyn TaskStore> = Arc::new(tasks.clone());
    let jobs: Arc<dyn JobQueue> = Arc::new(tasks.clone());
    let query: Arc<dyn TaskQuery> = Arc::new(tasks.clone());
    let watch: Arc<dyn TaskWatch> = Arc::new(tasks.clone());
    let node = roster.node.clone();
    let mesh = roster.mesh;
    let source: Arc<dyn CatalogSource> = catalog;
    let agents: Arc<dyn TaskAgents> = roster;
    Arc::new(gateway::routes::tasks::TasksApi {
        store,
        jobs,
        query,
        watch,
        catalog: source,
        agents,
        approvals: None,
        node,
        mesh,
        allowed_harnesses: allowed,
        command_timeout: gateway::routes::tasks::COMMAND_LIST_TIMEOUT,
    })
}

#[cfg(test)]
#[path = "ports_test.rs"]
mod tests;
