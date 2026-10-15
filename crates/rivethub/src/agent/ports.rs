use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use kernel::queue::JobQueue;
use kernel::tasks::CatalogSource;
use kernel::tasks::TaskAgents;
use kernel::tasks::TaskError;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskStore;
use kernel::tasks::TaskWatch;
use memory_postgres::PostgresTasks;
use wire::gateway::agents::AgentPreset;
use wire::harness::HarnessId;
use wire::mesh::MeshNode;
use wire::mesh::MeshNodeStatus;

use super::LocalBinding;
use super::assemble::Plan;

#[derive(Debug)]
pub(super) struct Roster {
    pub node: String,
    pub mesh: bool,
    pub local: BTreeMap<String, LocalBinding>,
    pub presets: Vec<AgentPreset>,
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
        for preset in &self.presets {
            directory.preset(preset.clone());
        }
        for node in &self.nodes {
            directory.mesh_node(node.clone());
        }
        directory.set_mesh_secret(plan.mesh_secret.clone());
        Ok(directory)
    }

    fn known(&self, agent_id: &str) -> bool {
        if self.local.contains_key(agent_id) {
            return true;
        }
        if self
            .presets
            .iter()
            .any(|preset| preset.name == agent_id || preset.id == agent_id)
        {
            return true;
        }
        self.nodes.iter().any(|node| {
            node.status == MeshNodeStatus::Online
                && node.agents.iter().any(|agent| agent == agent_id)
        })
    }
}

#[async_trait]
impl TaskAgents for Roster {
    async fn contains(&self, agent_id: &str) -> Result<bool, TaskError> {
        Ok(self.known(agent_id))
    }
}

pub(super) fn api(
    roster: Arc<Roster>,
    catalog: Arc<tasks::Catalog>,
    allowed: Option<Vec<HarnessId>>,
    tasks: &PostgresTasks,
) -> Arc<gateway::routes::tasks::TasksApi> {
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
