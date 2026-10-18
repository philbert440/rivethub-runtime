use std::sync::Arc;

use async_trait::async_trait;
use kernel::harness::HarnessAdapter;
use kernel::tasks::CatalogList;
use kernel::tasks::CatalogSource;
use kernel::tasks::TaskError;
use wire::gateway::agents::AgentPreset;
use wire::gateway::catalog::CatalogAgent;
use wire::gateway::catalog::CatalogCommand;
use wire::gateway::catalog::CatalogExecutorEntry;
use wire::gateway::catalog::CatalogNode;
use wire::gateway::catalog::CatalogPreset;
use wire::gateway::catalog::CatalogSkillEntry;
use wire::task::TaskExecutorKind;

use crate::directory::Directory;
use crate::executors;

pub struct Catalog {
    agents: Vec<CatalogAgent>,
    mesh: bool,
    tools: Vec<String>,
    harness: Option<Arc<dyn HarnessAdapter>>,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "the harness port is not Debug (kernel ports intentionally do not require Debug), so the impl omits it"
    )
)]
impl std::fmt::Debug for Catalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Catalog")
            .field("agents", &self.agents)
            .field("mesh", &self.mesh)
            .field("tools", &self.tools)
            .finish_non_exhaustive()
    }
}

impl Catalog {
    pub fn new(
        directory: &Directory,
        mesh: bool,
        tools: Vec<String>,
        harness: Option<Arc<dyn HarnessAdapter>>,
    ) -> Self {
        Self {
            agents: listed_agents(directory),
            mesh,
            tools,
            harness,
        }
    }
}

fn listed_agents(directory: &Directory) -> Vec<CatalogAgent> {
    let node = directory.local_node();
    let mut agents = Vec::new();
    for preset in directory.presets() {
        agents.push(CatalogAgent::Preset(preset_entry(node, preset)));
    }
    for (id, agent) in directory.local_entries() {
        agents.push(CatalogAgent::Node(CatalogNode {
            id: id.to_owned(),
            node: node.to_owned(),
            local: true,
            provider: agent.provider.clone(),
            model: agent.model.clone(),
        }));
    }
    agents
}

fn preset_entry(local_node: &str, preset: &AgentPreset) -> CatalogPreset {
    let (node, local) = match preset.node.as_deref() {
        Some(node) if node == local_node => (node.to_owned(), true),
        Some(node) => (node.to_owned(), false),
        None => (local_node.to_owned(), true),
    };
    CatalogPreset {
        id: preset.id.clone(),
        name: preset.name.clone(),
        node,
        local,
        harness_id: preset.harness_id,
        model: Some(preset.model.clone()),
        directory: preset.directory.clone(),
        implemented: Some(true),
        gap: None,
    }
}

fn executor(kind: TaskExecutorKind) -> CatalogExecutorEntry {
    CatalogExecutorEntry {
        key: kind.to_string(),
        capabilities: executors::capabilities(kind),
        commands: Vec::new(),
        harness_id: None,
        implemented: Some(true),
    }
}

#[async_trait]
impl CatalogSource for Catalog {
    async fn agents(&self) -> Result<Vec<CatalogAgent>, TaskError> {
        Ok(self.agents.clone())
    }

    async fn executors(&self) -> Result<Vec<CatalogExecutorEntry>, TaskError> {
        Ok(executors::offered(self.mesh)
            .into_iter()
            .map(executor)
            .collect())
    }

    async fn tools(&self) -> Result<Vec<String>, TaskError> {
        Ok(self.tools.clone())
    }

    async fn skills(&self) -> Result<Vec<CatalogSkillEntry>, TaskError> {
        Ok(Vec::new())
    }

    async fn commands(&self, executor: &str) -> Result<Vec<CatalogCommand>, TaskError> {
        let kind = executor
            .parse::<TaskExecutorKind>()
            .map_err(|_| TaskError::UnavailableCatalog(CatalogList::Commands))?;
        let harness = match (kind, &self.harness) {
            (TaskExecutorKind::HarnessSession, Some(harness)) => harness,
            (TaskExecutorKind::HarnessSession, None)
            | (TaskExecutorKind::ChatLoop | TaskExecutorKind::Mesh, _) => return Ok(Vec::new()),
        };
        let discovered = harness
            .commands()
            .await
            .map_err(|error| TaskError::Operation {
                operation: "harness commands",
                source: Box::new(error),
            })?;
        Ok(discovered
            .into_iter()
            .flat_map(|listed| {
                let source = listed.harness.to_string();
                listed
                    .commands
                    .into_iter()
                    .map(move |command| CatalogCommand {
                        name: command.name,
                        description: command.description,
                        arg_hint: None,
                        source: source.clone(),
                    })
            })
            .collect())
    }
}
