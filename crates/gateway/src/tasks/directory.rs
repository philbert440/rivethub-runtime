use std::collections::BTreeMap;

use chrono::DateTime;
use chrono::Utc;
use wire::gateway::agents::AgentPreset;
use wire::gateway::catalog::CatalogAgent;
use wire::gateway::catalog::CatalogNode;
use wire::gateway::catalog::CatalogPreset;
use wire::harness::HarnessId;
use wire::mesh::MeshNode;
use wire::mesh::MeshNodeStatus;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalAgent {
    pub provider: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Sources<'a> {
    pub(crate) node: &'a str,
    pub(crate) presets: Vec<AgentPreset>,
    pub(crate) local: &'a BTreeMap<String, LocalAgent>,
    pub(crate) harnessed: BTreeMap<String, HarnessId>,
    pub(crate) nodes: Option<Vec<MeshNode>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resolved {
    pub(crate) agent: String,
    pub(crate) preset: Option<String>,
    pub(crate) node: String,
    pub(crate) harness: Option<HarnessId>,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum Refusal {
    #[error("agentId (string) is required")]
    AgentRequired,
    #[error("nodeAffinity is empty")]
    EmptyNode,
    #[error("{target} names node {embedded} but nodeAffinity is {explicit}")]
    ConflictingNode {
        target: String,
        embedded: String,
        explicit: String,
    },
    #[error("agent \"{0}\" matches more than one preset")]
    Ambiguous(String),
    #[error("agent \"{0}\" is not a preset or a runtime agent on an online node")]
    Unknown(String),
    #[error("agent \"{agent}\" is not on node {node}")]
    NotOnNode { agent: String, node: String },
    #[error("mesh node {0} is not online")]
    Offline(String),
    #[error("agent preset \"{0}\" has no harness; give it one to delegate to it")]
    PresetHarness(String),
    #[error(
        "agent \"{0}\" runs on a model provider, not a harness, so work can't be delegated to it; delegate to an opencode or pi preset on that model instead"
    )]
    NotHarnessed(String),
}

struct Host {
    node: String,
    seen: Option<DateTime<Utc>>,
}

impl Sources<'_> {
    pub(crate) fn resolve(&self, target: &str, pin: Option<&str>) -> Result<Resolved, Refusal> {
        if target.is_empty() {
            return Err(Refusal::AgentRequired);
        }
        if pin.is_some_and(str::is_empty) {
            return Err(Refusal::EmptyNode);
        }
        if target.contains('@')
            && let Some(preset) = self.one_preset(target)?
        {
            return self.preset(preset, pin);
        }
        let (agent, embedded) = split(target)?;
        let pin = chosen(target, embedded, pin)?;
        let preset = self.one_preset(agent)?;
        let hosts = self.hosts(agent);
        match pin {
            None => match preset {
                Some(preset) if preset.harness_id.is_some() || hosts.is_empty() => {
                    self.preset(preset, None)
                }
                _ => match newest(&hosts) {
                    Some(host) => self.runtime(agent, &host.node),
                    None => Err(Refusal::Unknown(agent.to_owned())),
                },
            },
            Some(pin) if hosts.iter().any(|host| host.node == pin) => self.runtime(agent, pin),
            Some(pin) => match preset {
                Some(preset) => self.preset(preset, Some(pin)),
                None if hosts.is_empty() => Err(Refusal::Unknown(agent.to_owned())),
                None => Err(Refusal::NotOnNode {
                    agent: agent.to_owned(),
                    node: pin.to_owned(),
                }),
            },
        }
    }

    pub(crate) fn catalog(&self) -> Vec<CatalogAgent> {
        let mut agents: Vec<CatalogAgent> = self
            .presets
            .iter()
            .map(|preset| CatalogAgent::Preset(self.preset_entry(preset)))
            .collect();
        for (id, agent) in self.local {
            agents.push(CatalogAgent::Node(CatalogNode {
                id: id.clone(),
                node: self.node.to_owned(),
                local: true,
                provider: agent.provider.clone(),
                model: agent.model.clone(),
            }));
        }
        for node in self.online().filter(|node| node.id != self.node) {
            for agent in &node.agents {
                agents.push(CatalogAgent::Node(CatalogNode {
                    id: agent.clone(),
                    node: node.id.clone(),
                    local: false,
                    provider: None,
                    model: None,
                }));
            }
        }
        agents
    }

    fn preset(&self, preset: &AgentPreset, pin: Option<&str>) -> Result<Resolved, Refusal> {
        let node = pin
            .or(preset.node.as_deref())
            .unwrap_or(self.node)
            .to_owned();
        self.reachable(&node)?;
        let harness = preset
            .harness_id
            .ok_or_else(|| Refusal::PresetHarness(preset.name.clone()))?;
        Ok(Resolved {
            agent: preset.name.clone(),
            preset: Some(preset.id.clone()),
            node,
            harness: Some(harness),
            model: preset.launch_model().map(std::borrow::Cow::into_owned),
            effort: Some(preset.effort.clone()).filter(|effort| !effort.is_empty()),
        })
    }

    fn runtime(&self, agent: &str, node: &str) -> Result<Resolved, Refusal> {
        self.reachable(node)?;
        let harness = if node == self.node {
            Some(
                *self
                    .harnessed
                    .get(agent)
                    .ok_or_else(|| Refusal::NotHarnessed(agent.to_owned()))?,
            )
        } else {
            None
        };
        Ok(Resolved {
            agent: agent.to_owned(),
            preset: None,
            node: node.to_owned(),
            harness,
            model: None,
            effort: None,
        })
    }

    fn reachable(&self, node: &str) -> Result<(), Refusal> {
        if node == self.node {
            return Ok(());
        }
        match &self.nodes {
            None => Ok(()),
            Some(_) if self.online().any(|known| known.id == node) => Ok(()),
            Some(_) => Err(Refusal::Offline(node.to_owned())),
        }
    }

    fn one_preset(&self, key: &str) -> Result<Option<&AgentPreset>, Refusal> {
        let mut found = self
            .presets
            .iter()
            .filter(|preset| preset.name == key || preset.id == key);
        let first = found.next();
        if found.next().is_some() {
            return Err(Refusal::Ambiguous(key.to_owned()));
        }
        Ok(first)
    }

    fn hosts(&self, agent: &str) -> Vec<Host> {
        let mut hosts = Vec::new();
        if self.local.contains_key(agent) {
            hosts.push(Host {
                node: self.node.to_owned(),
                seen: self
                    .online()
                    .find(|node| node.id == self.node)
                    .map(|node| node.last_seen),
            });
        }
        for node in self.online().filter(|node| node.id != self.node) {
            if node.agents.iter().any(|name| name == agent) {
                hosts.push(Host {
                    node: node.id.clone(),
                    seen: Some(node.last_seen),
                });
            }
        }
        hosts
    }

    fn online(&self) -> impl Iterator<Item = &MeshNode> {
        self.nodes
            .iter()
            .flatten()
            .filter(|node| node.status == MeshNodeStatus::Online)
    }

    fn preset_entry(&self, preset: &AgentPreset) -> CatalogPreset {
        let node = preset.node.clone().unwrap_or_else(|| self.node.to_owned());
        CatalogPreset {
            local: node == self.node,
            node,
            id: preset.id.clone(),
            name: preset.name.clone(),
            harness_id: preset.harness_id,
            model: Some(preset.model.clone()),
            effort: Some(preset.effort.clone()),
            directory: preset.directory.clone(),
            implemented: Some(true),
            gap: None,
        }
    }
}

fn split(target: &str) -> Result<(&str, Option<&str>), Refusal> {
    match target.rsplit_once('@') {
        None => Ok((target, None)),
        Some((agent, node)) if !agent.is_empty() && !node.is_empty() => Ok((agent, Some(node))),
        Some(_) => Err(Refusal::AgentRequired),
    }
}

fn chosen<'a>(
    target: &str,
    embedded: Option<&'a str>,
    explicit: Option<&'a str>,
) -> Result<Option<&'a str>, Refusal> {
    match (embedded, explicit) {
        (Some(embedded), Some(explicit)) if embedded != explicit => Err(Refusal::ConflictingNode {
            target: target.to_owned(),
            embedded: embedded.to_owned(),
            explicit: explicit.to_owned(),
        }),
        (Some(node), _) | (None, Some(node)) => Ok(Some(node)),
        (None, None) => Ok(None),
    }
}

fn newest(hosts: &[Host]) -> Option<&Host> {
    hosts
        .iter()
        .max_by(|left, right| left.seen.cmp(&right.seen).then(right.node.cmp(&left.node)))
}

#[cfg(test)]
#[path = "directory_test.rs"]
mod tests;
