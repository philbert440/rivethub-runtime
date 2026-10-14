use std::collections::BTreeMap;
use std::net::Ipv6Addr;

use url::Url;
use wire::gateway::agents::AgentPreset;
use wire::harness::HarnessId;
use wire::mesh::MeshNode;
use wire::mesh::MeshNodeStatus;
use wire::task::Effort;
use wire::task::TaskExecutorKind;
use wire::task::TaskSpec;

use crate::error::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalAgent {
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub harness: Option<HarnessId>,
    pub system_prompt: Option<String>,
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Directory {
    local_node: String,
    gateway: Url,
    local_agents: BTreeMap<String, LocalAgent>,
    presets: BTreeMap<String, AgentPreset>,
    nodes: BTreeMap<String, MeshNode>,
    mesh_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInput {
    pub executor: TaskExecutorKind,
    pub executor_target: Option<String>,
    pub node_affinity: Option<String>,
    pub agent_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prepared {
    pub node: String,
    pub remote: bool,
    pub job_name: kernel::queue::JobName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub executor: TaskExecutorKind,
    pub node: String,
    pub base: Url,
    pub bearer: Option<String>,
    pub harness: Option<HarnessId>,
    pub model: Option<String>,
    pub effort: Option<Effort>,
    pub system_prompt: Option<String>,
    pub working_dir: Option<String>,
    pub remote: bool,
}

impl Directory {
    pub fn new(local_node: impl Into<String>, gateway: Url) -> Result<Self, Error> {
        let local_node = local_node.into();
        if local_node.is_empty() || gateway.host_str().is_none() {
            return Err(Error::NodeRequired);
        }
        Ok(Self {
            local_node,
            gateway,
            local_agents: BTreeMap::new(),
            presets: BTreeMap::new(),
            nodes: BTreeMap::new(),
            mesh_secret: None,
        })
    }

    pub fn local_agent(&mut self, name: impl Into<String>, agent: LocalAgent) {
        self.local_agents.insert(name.into(), agent);
    }

    pub fn preset(&mut self, preset: AgentPreset) {
        self.presets.insert(preset.name.clone(), preset);
    }

    pub fn mesh_node(&mut self, node: MeshNode) {
        self.nodes.insert(node.id.clone(), node);
    }

    pub fn set_mesh_secret(&mut self, secret: Option<String>) {
        self.mesh_secret = secret;
    }

    pub fn prepare(&self, input: &TargetInput) -> Result<Prepared, Error> {
        let node = self.target_node(input)?;
        let remote = node != self.local_node;
        if remote {
            let mesh = self.roster(&node, &input.agent_id)?;
            let base = mesh_base(mesh, self.preset_base(&input.agent_id))?;
            drop(mesh_bearer_for(&base, self.mesh_secret.as_deref())?);
        } else {
            drop(self.resolve(input)?);
        }
        Ok(Prepared {
            job_name: job_name(&node, remote)?,
            node,
            remote,
        })
    }

    pub fn resolve(&self, input: &TargetInput) -> Result<ResolvedTarget, Error> {
        let node = self.target_node(input)?;
        if node != self.local_node || input.executor == TaskExecutorKind::Mesh {
            return self.resolve_mesh(input, &node);
        }
        self.resolve_local(input, &node)
    }

    fn target_node(&self, input: &TargetInput) -> Result<String, Error> {
        if input.executor == TaskExecutorKind::Mesh {
            return mesh_node_name(input);
        }
        if let Some(preset) = self.presets.get(&input.agent_id)
            && let Some(node) = preset.node.as_ref()
            && node != &self.local_node
        {
            return Ok(node.clone());
        }
        match input.node_affinity.as_ref() {
            Some(node) if node != &self.local_node => Ok(node.clone()),
            Some(_) | None => Ok(self.local_node.clone()),
        }
    }

    fn resolve_local(&self, input: &TargetInput, node: &str) -> Result<ResolvedTarget, Error> {
        let local = self.local_agents.get(&input.agent_id);
        let preset = self.presets.get(&input.agent_id);
        let fields = match (local, preset) {
            (Some(_), Some(_)) => {
                return Err(Error::AmbiguousTarget {
                    agent: input.agent_id.clone(),
                });
            }
            (Some(agent), None) => fields_from_local(agent),
            (None, Some(preset)) => fields_from_preset(preset)?,
            (None, None) => {
                return Err(Error::UnknownTarget {
                    agent: input.agent_id.clone(),
                });
            }
        };
        Ok(resolved(ResolvedParts {
            input,
            node,
            base: self.gateway.clone(),
            bearer: None,
            remote: false,
            fields,
        }))
    }

    fn resolve_mesh(&self, input: &TargetInput, node: &str) -> Result<ResolvedTarget, Error> {
        let mesh = self.roster(node, &input.agent_id)?;
        let preset = self.presets.get(&input.agent_id);
        let local = self.local_agents.get(&input.agent_id);
        let base_override = preset.and_then(|item| item.node_base_url.as_deref());
        let base = mesh_base(mesh, base_override)?;
        let fields = if let Some(preset) = preset {
            fields_from_preset(preset)?
        } else if let Some(agent) = local {
            fields_from_local(agent)
        } else {
            AgentFields::default()
        };
        let bearer = mesh_bearer_for(&base, self.mesh_secret.as_deref())?;
        Ok(resolved(ResolvedParts {
            input,
            node,
            base,
            bearer,
            remote: true,
            fields,
        }))
    }

    fn preset_base(&self, agent: &str) -> Option<&str> {
        self.presets
            .get(agent)
            .and_then(|item| item.node_base_url.as_deref())
    }

    fn roster<'a>(&'a self, node: &str, agent: &str) -> Result<&'a MeshNode, Error> {
        let mesh = self.nodes.get(node).ok_or_else(|| Error::MeshUnavailable {
            node: node.to_string(),
        })?;
        if mesh.status != MeshNodeStatus::Online {
            return Err(Error::MeshUnavailable {
                node: node.to_string(),
            });
        }
        if !mesh.agents.iter().any(|name| name == agent) {
            return Err(Error::MeshAgent {
                node: node.to_string(),
                agent: agent.to_string(),
            });
        }
        Ok(mesh)
    }
}

struct ResolvedParts<'a> {
    input: &'a TargetInput,
    node: &'a str,
    base: Url,
    bearer: Option<String>,
    remote: bool,
    fields: AgentFields,
}

fn resolved(parts: ResolvedParts<'_>) -> ResolvedTarget {
    ResolvedTarget {
        executor: parts.input.executor,
        node: parts.node.to_string(),
        base: parts.base,
        bearer: parts.bearer,
        harness: parts.fields.harness,
        model: parts.fields.model,
        effort: parts.fields.effort,
        system_prompt: parts.fields.system_prompt,
        working_dir: parts.fields.working_dir,
        remote: parts.remote,
    }
}

#[derive(Debug, Default)]
struct AgentFields {
    harness: Option<HarnessId>,
    model: Option<String>,
    effort: Option<Effort>,
    system_prompt: Option<String>,
    working_dir: Option<String>,
}

fn fields_from_local(agent: &LocalAgent) -> AgentFields {
    AgentFields {
        harness: agent.harness,
        model: agent.model.clone(),
        effort: agent.effort,
        system_prompt: agent.system_prompt.clone(),
        working_dir: agent.working_dir.clone(),
    }
}

fn fields_from_preset(preset: &AgentPreset) -> Result<AgentFields, Error> {
    Ok(AgentFields {
        harness: preset.harness_id,
        model: Some(preset.model.clone()),
        effort: Some(parse_effort(&preset.effort)?),
        system_prompt: Some(preset.system_prompt.clone()),
        working_dir: preset.directory.clone(),
    })
}

fn parse_effort(text: &str) -> Result<Effort, Error> {
    crate::payload::from_text(text)
}

pub(crate) fn apply_spec(target: &mut ResolvedTarget, spec: &TaskSpec) -> Result<(), Error> {
    if let Some(model) = &spec.model {
        target.model = Some(model.clone());
    }
    if let Some(effort) = spec.effort {
        target.effort = Some(effort);
    }
    if let Some(extra) = &spec.system_prompt_append {
        target.system_prompt = Some(match target.system_prompt.as_deref() {
            Some(existing) => format!("{existing}\n{extra}"),
            None => extra.clone(),
        });
    }
    if let Some(working) = &spec.working_dir {
        target.working_dir = Some(working.clone());
    }
    if target.executor == TaskExecutorKind::HarnessSession && target.harness.is_none() {
        return Err(Error::HarnessRequired);
    }
    Ok(())
}

fn mesh_node_name(input: &TargetInput) -> Result<String, Error> {
    match input
        .executor_target
        .as_ref()
        .or(input.node_affinity.as_ref())
    {
        Some(node) if !node.is_empty() => Ok(node.clone()),
        Some(_) | None => Err(Error::MeshAddress),
    }
}

fn job_name(node: &str, remote: bool) -> Result<kernel::queue::JobName, Error> {
    if remote {
        if node.is_empty() {
            return Err(Error::JobName(kernel::queue::JobNameError::EmptyNode));
        }
        Ok(kernel::queue::JobName::RunTaskOn {
            node: node.to_string(),
        })
    } else {
        Ok(kernel::queue::JobName::Fixed(
            kernel::queue::FixedJob::RunTask,
        ))
    }
}

fn mesh_base(node: &MeshNode, preset_base: Option<&str>) -> Result<Url, Error> {
    if let Some(base) = preset_base {
        return parse_base(base);
    }
    if node.host.is_empty() || node.host.contains(['/', '\\', ' ', '@', '?']) {
        return Err(Error::MeshAddress);
    }
    let host = bracket_host(&node.host)?;
    let url =
        Url::parse(&format!("https://{host}:{}/", node.port)).map_err(|_| Error::MeshAddress)?;
    let parsed = url.host_str().ok_or(Error::MeshAddress)?;
    let expected = node.host.trim_matches(['[', ']']);
    if parsed.trim_matches(['[', ']']) != expected {
        return Err(Error::MeshAddress);
    }
    Ok(url)
}

fn mesh_bearer_for(base: &Url, secret: Option<&str>) -> Result<Option<String>, Error> {
    if base.scheme() == "http" {
        return Ok(None);
    }
    if base.scheme() != "https" {
        return Err(Error::MeshAddress);
    }
    match secret {
        Some(secret) if !secret.is_empty() => Ok(Some(secret.to_string())),
        Some(_) | None => Err(Error::MeshUnauthorized),
    }
}

fn parse_base(base: &str) -> Result<Url, Error> {
    if base.is_empty() {
        return Err(Error::BaseUrl);
    }
    let url = Url::parse(base).map_err(|_| Error::BaseUrl)?;
    if url.host_str().is_none() || url.username() != "" || url.password().is_some() {
        return Err(Error::BaseUrl);
    }
    Ok(url)
}

fn bracket_host(host: &str) -> Result<String, Error> {
    let bare = host.trim_matches(['[', ']']);
    if bare.is_empty() {
        return Err(Error::MeshAddress);
    }
    if bare.parse::<Ipv6Addr>().is_ok() {
        Ok(format!("[{bare}]"))
    } else if host.contains(':') {
        Err(Error::MeshAddress)
    } else {
        Ok(bare.to_string())
    }
}
