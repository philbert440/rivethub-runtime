use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use kernel::tasks::ApprovalBroker;
use kernel::tasks::ApprovalDecision;
use kernel::tasks::ApprovalError;
use kernel::tasks::CatalogSource;
use kernel::tasks::TaskAgents;
use kernel::tasks::TaskError;
use kernel::tasks::TaskJobs;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskStore;
use kernel::tasks::TaskWatch;
use memory_postgres::PostgresTasks;
use uuid::Uuid;
use wire::gateway::agents::AgentPreset;
use wire::gateway::catalog::CatalogAgent;
use wire::gateway::catalog::CatalogCommand;
use wire::gateway::catalog::CatalogExecutorEntry;
use wire::gateway::catalog::CatalogNode;
use wire::gateway::catalog::CatalogPreset;
use wire::gateway::catalog::CatalogSkillEntry;
use wire::harness::HarnessId;
use wire::mesh::MeshNode;
use wire::mesh::MeshNodeStatus;
use wire::task::ExecutorInteractionCapabilities;
use wire::task::ExecutorResultCapabilities;
use wire::task::ExecutorTurnCapabilities;
use wire::task::HarnessExecutorCapabilities;
use wire::task::McpInjection;
use wire::task::TaskApprovalRequest;
use wire::task::TaskExecutorKind;

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

    fn catalog_agents(&self) -> Vec<CatalogAgent> {
        let mut presets = self.presets.clone();
        presets.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        let mut agents = Vec::with_capacity(presets.len() + self.local.len());
        for preset in presets {
            agents.push(CatalogAgent::Preset(preset_entry(&self.node, preset)));
        }
        for (id, binding) in &self.local {
            agents.push(CatalogAgent::Node(CatalogNode {
                id: id.clone(),
                node: self.node.clone(),
                local: true,
                provider: binding.provider.clone(),
                model: binding.model.clone(),
            }));
        }
        agents
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

fn preset_entry(local_node: &str, preset: AgentPreset) -> CatalogPreset {
    let (node, local) = match preset.node {
        Some(node) if node == local_node => (node, true),
        Some(node) => (node, false),
        None => (local_node.to_owned(), true),
    };
    CatalogPreset {
        id: preset.id,
        name: preset.name,
        node,
        local,
        harness_id: preset.harness_id,
        model: Some(preset.model),
        directory: preset.directory,
        implemented: Some(true),
        gap: None,
    }
}

fn executor(kind: TaskExecutorKind) -> CatalogExecutorEntry {
    CatalogExecutorEntry {
        key: kind.to_string(),
        capabilities: capabilities(),
        commands: Vec::new(),
        harness_id: None,
        implemented: Some(true),
    }
}

fn capabilities() -> HarnessExecutorCapabilities {
    HarnessExecutorCapabilities {
        turn: ExecutorTurnCapabilities {
            steerable: true,
            multi_turn: true,
            structured_stream: false,
        },
        result: ExecutorResultCapabilities {
            usage_in_result: true,
            session_id_capture: true,
        },
        interaction: ExecutorInteractionCapabilities {
            slash_commands: false,
            effort_selection: false,
        },
        mcp_injection: McpInjection::None,
    }
}

#[async_trait]
impl CatalogSource for Roster {
    async fn agents(&self) -> Result<Vec<CatalogAgent>, TaskError> {
        Ok(self.catalog_agents())
    }

    async fn executors(&self) -> Result<Vec<CatalogExecutorEntry>, TaskError> {
        let mut entries = vec![
            executor(TaskExecutorKind::ChatLoop),
            executor(TaskExecutorKind::HarnessSession),
        ];
        if self.mesh {
            entries.push(executor(TaskExecutorKind::Mesh));
        }
        Ok(entries)
    }

    async fn tools(&self) -> Result<Vec<String>, TaskError> {
        Ok(Vec::new())
    }

    async fn skills(&self) -> Result<Vec<CatalogSkillEntry>, TaskError> {
        Ok(Vec::new())
    }

    async fn commands(&self, _executor: &str) -> Result<Vec<CatalogCommand>, TaskError> {
        Ok(Vec::new())
    }
}

#[async_trait]
impl TaskAgents for Roster {
    async fn contains(&self, agent_id: &str) -> Result<bool, TaskError> {
        Ok(self.known(agent_id))
    }
}

#[derive(Debug, Default)]
struct EmptyApprovals;

#[async_trait]
impl ApprovalBroker for EmptyApprovals {
    async fn pending(&self, _task: Uuid) -> Result<Vec<TaskApprovalRequest>, ApprovalError> {
        Ok(Vec::new())
    }

    async fn resolve(
        &self,
        _task: Uuid,
        _request_id: &str,
        _decision: ApprovalDecision,
    ) -> Result<(), ApprovalError> {
        Err(ApprovalError::Unknown)
    }

    async fn deny_pending(&self, _task: Uuid, _reason: &str) -> Result<(), ApprovalError> {
        Ok(())
    }
}

pub(super) fn api(
    roster: Arc<Roster>,
    allowed: Option<Vec<HarnessId>>,
    tasks: &PostgresTasks,
) -> Arc<gateway::routes::tasks::TasksApi> {
    let store: Arc<dyn TaskStore> = Arc::new(tasks.clone());
    let jobs: Arc<dyn TaskJobs> = Arc::new(tasks.clone());
    let query: Arc<dyn TaskQuery> = Arc::new(tasks.clone());
    let watch: Arc<dyn TaskWatch> = Arc::new(tasks.clone());
    let node = roster.node.clone();
    let mesh = roster.mesh;
    let catalog: Arc<dyn CatalogSource> = roster.clone();
    let agents: Arc<dyn TaskAgents> = roster;
    Arc::new(gateway::routes::tasks::TasksApi {
        store,
        jobs,
        query,
        watch,
        catalog,
        agents,
        approvals: Some(Arc::new(EmptyApprovals)),
        node,
        mesh,
        allowed_harnesses: allowed,
        command_timeout: gateway::routes::tasks::COMMAND_LIST_TIMEOUT,
    })
}
