use std::collections::BTreeSet;
use std::time::Duration;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::queue::JobQueue;
use kernel::tasks::AgentDirectory;
use kernel::tasks::AgentNode;
use kernel::tasks::DelegateOutcome;
use kernel::tasks::DelegateRequest;
use kernel::tasks::Delegation;
use kernel::tasks::DelegationError;
use kernel::tasks::PresetAgent;
use kernel::tasks::RuntimeAgent;
use kernel::tasks::TaskOrigin;
use kernel::tasks::TaskRecord;
use kernel::tasks::TaskStore;
use kernel::tasks::terminal;
use tokio_util::sync::CancellationToken;
use wire::gateway::agents::AgentPreset;
use wire::task::TaskBudget;
use wire::task::TaskCreateRequest;
use wire::task::TaskExecutorKind;
use wire::task_overrides::{TaskColumns, TaskOverrides};

use crate::directory::Directory;
use crate::directory::TargetInput;
use crate::engine::Engine;
use crate::error::Error;
use crate::submit::NewTask;

struct Host {
    id: String,
    last_seen: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub struct DelegationTarget {
    pub input: TargetInput,
    preset: Option<(String, String)>,
}

impl Directory {
    pub fn catalog(&self) -> AgentDirectory {
        let mut presets: Vec<PresetAgent> = self
            .presets()
            .map(|preset| preset_agent(preset, self.local_node()))
            .collect();
        presets.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut ids = BTreeSet::new();
        for name in self.local_agent_names() {
            ids.insert(name.to_owned());
        }
        for node in self.online_nodes() {
            if node.id == self.local_node() {
                continue;
            }
            ids.extend(node.agents.iter().cloned());
        }
        let runtime = ids
            .into_iter()
            .map(|id| RuntimeAgent {
                nodes: agent_nodes(self, &id),
                id,
            })
            .collect();
        AgentDirectory { presets, runtime }
    }

    pub fn delegation_target(
        &self,
        target: &str,
        node: Option<&str>,
    ) -> Result<DelegationTarget, Error> {
        if target.is_empty() {
            return Err(Error::AgentRequired);
        }
        if matches!(node, Some(explicit) if explicit.is_empty()) {
            return Err(Error::MeshAddress);
        }
        if target.contains('@') {
            let matches = presets_for(self, target);
            if !matches.is_empty() {
                return preset_input(target, &matches, node.map(ToOwned::to_owned));
            }
        }
        let (agent, embedded) = split_target(target)?;
        let pin = chosen_node(target, embedded, node)?;
        let preset = one_preset(&agent, &presets_for(self, &agent))?;
        let hosts = hosts(self, &agent);
        match pin {
            None => open_target(&agent, preset, &hosts),
            Some(pin) => pinned_target(&agent, preset, &hosts, pin),
        }
    }
}

fn preset_agent(preset: &AgentPreset, local_node: &str) -> PresetAgent {
    PresetAgent {
        id: preset.id.clone(),
        name: preset.name.clone(),
        node: preset.node.clone(),
        harness: preset.harness_id,
        directory: preset.directory.clone(),
        local: preset.node.as_deref().is_none_or(|node| node == local_node),
    }
}

fn agent_nodes(directory: &Directory, agent: &str) -> Vec<AgentNode> {
    let mut nodes = hosts(directory, agent);
    nodes.sort_by(|left, right| {
        right
            .last_seen
            .cmp(&left.last_seen)
            .then_with(|| left.id.cmp(&right.id))
    });
    nodes
        .into_iter()
        .map(|host| AgentNode {
            id: host.id,
            last_seen: host.last_seen,
        })
        .collect()
}

fn presets_for<'a>(directory: &'a Directory, key: &str) -> Vec<&'a AgentPreset> {
    directory
        .presets()
        .filter(|preset| preset.name == key || preset.id == key)
        .collect()
}

fn one_preset<'a>(
    key: &str,
    matches: &[&'a AgentPreset],
) -> Result<Option<&'a AgentPreset>, Error> {
    match matches {
        [] => Ok(None),
        [preset] => Ok(Some(*preset)),
        _ => Err(Error::AmbiguousTarget {
            agent: key.to_owned(),
        }),
    }
}

fn preset_input(
    key: &str,
    matches: &[&AgentPreset],
    affinity: Option<String>,
) -> Result<DelegationTarget, Error> {
    let preset = one_preset(key, matches)?.ok_or_else(|| Error::UnknownTarget {
        agent: key.to_owned(),
    })?;
    Ok(preset_target(preset, affinity))
}

fn open_target(
    agent: &str,
    preset: Option<&AgentPreset>,
    hosts: &[Host],
) -> Result<DelegationTarget, Error> {
    if let Some(preset) = preset
        && (preset.harness_id.is_some() || hosts.is_empty())
    {
        return Ok(preset_target(preset, None));
    }
    match newest(hosts) {
        Some(host) => Ok(runtime_target(agent, &host.id)),
        None => Err(Error::UnknownTarget {
            agent: agent.to_owned(),
        }),
    }
}

fn pinned_target(
    agent: &str,
    preset: Option<&AgentPreset>,
    hosts: &[Host],
    pin: String,
) -> Result<DelegationTarget, Error> {
    if hosts.iter().any(|host| host.id == pin) {
        return Ok(runtime_target(agent, &pin));
    }
    if let Some(preset) = preset {
        return Ok(preset_target(preset, Some(pin)));
    }
    if hosts.is_empty() {
        Err(Error::UnknownTarget {
            agent: agent.to_owned(),
        })
    } else {
        Err(Error::MeshAgent {
            node: pin,
            agent: agent.to_owned(),
        })
    }
}

fn preset_target(preset: &AgentPreset, affinity: Option<String>) -> DelegationTarget {
    let executor = match preset.harness_id {
        Some(_) => TaskExecutorKind::HarnessSession,
        None => TaskExecutorKind::ChatLoop,
    };
    DelegationTarget {
        preset: Some((preset.id.clone(), preset.name.clone())),
        input: TargetInput {
            executor,
            executor_target: None,
            node_affinity: affinity,
            agent_id: preset.name.clone(),
        },
    }
}

fn runtime_target(agent: &str, node: &str) -> DelegationTarget {
    DelegationTarget {
        preset: None,
        input: TargetInput {
            executor: TaskExecutorKind::ChatLoop,
            executor_target: None,
            node_affinity: Some(node.to_owned()),
            agent_id: agent.to_owned(),
        },
    }
}

fn split_target(target: &str) -> Result<(String, Option<String>), Error> {
    match target.rsplit_once('@') {
        None => Ok((target.to_owned(), None)),
        Some((agent, node)) if !agent.is_empty() && !node.is_empty() => {
            Ok((agent.to_owned(), Some(node.to_owned())))
        }
        Some(_) => Err(Error::AgentRequired),
    }
}

fn chosen_node(
    target: &str,
    embedded: Option<String>,
    explicit: Option<&str>,
) -> Result<Option<String>, Error> {
    match (embedded, explicit) {
        (Some(embedded), Some(explicit)) if embedded != explicit => Err(Error::ConflictingNode {
            target: target.to_owned(),
            embedded,
            explicit: explicit.to_owned(),
        }),
        (Some(embedded), Some(_) | None) => Ok(Some(embedded)),
        (None, Some(explicit)) => Ok(Some(explicit.to_owned())),
        (None, None) => Ok(None),
    }
}

fn hosts(directory: &Directory, agent: &str) -> Vec<Host> {
    let mut found = Vec::new();
    if directory.has_local_agent(agent) {
        let last_seen = directory
            .online_nodes()
            .find_map(|node| (node.id == directory.local_node()).then_some(node.last_seen));
        found.push(Host {
            id: directory.local_node().to_owned(),
            last_seen,
        });
    }
    for node in directory.online_nodes() {
        if node.id != directory.local_node() && node.agents.iter().any(|name| name == agent) {
            found.push(Host {
                id: node.id.clone(),
                last_seen: Some(node.last_seen),
            });
        }
    }
    found
}

fn newest(hosts: &[Host]) -> Option<&Host> {
    hosts.iter().max_by(|left, right| {
        left.last_seen
            .cmp(&right.last_seen)
            .then_with(|| right.id.cmp(&left.id))
    })
}

#[async_trait]
impl<S, J> Delegation for Engine<S, J>
where
    S: TaskStore + 'static,
    J: JobQueue + 'static,
{
    async fn agents(&self) -> Result<AgentDirectory, DelegationError> {
        Ok(self.directory().await.map_err(classify)?.catalog())
    }

    async fn delegate(
        &self,
        request: DelegateRequest,
        cancel: CancellationToken,
    ) -> Result<DelegateOutcome, DelegationError> {
        if cancel.is_cancelled() {
            return Err(DelegationError::Cancelled);
        }
        if request.timeout.is_zero() {
            return Err(classify(Error::TimeoutRange));
        }
        let directory = self.directory().await.map_err(classify)?;
        let target = directory
            .delegation_target(&request.target, request.node.as_deref())
            .map_err(classify)?;
        let prepared = directory.prepare(&target.input).map_err(classify)?;
        let origin = if prepared.remote {
            TaskOrigin::Mesh
        } else {
            TaskOrigin::Tool
        };
        let created = task_request(&request, target, directory.local_node());
        let id = self
            .submit(NewTask {
                request: created,
                origin,
            })
            .await
            .map_err(classify)?;
        if cancel.is_cancelled() {
            return Err(DelegationError::Cancelled);
        }
        self.wait(id, request.timeout, cancel).await
    }
}

fn task_request(
    request: &DelegateRequest,
    target: DelegationTarget,
    local_node: &str,
) -> TaskCreateRequest {
    let input = target.input;
    let budget = TaskBudget::default();
    let mut overrides = TaskOverrides {
        model: request.model.clone(),
        exclude_tools: Some(vec!["delegate_task".to_owned()]),
        ..TaskOverrides::default()
    };
    if let Some((id, name)) = target.preset {
        overrides.preset_id = Some(id);
        overrides.preset_name = Some(name);
    } else {
        overrides.delegation = Some(true);
        overrides.mesh_from = Some(local_node.to_owned());
    }
    let spec = overrides.into_spec(TaskColumns {
        task_id: String::new(),
        agent_id: input.agent_id.clone(),
        goal: request.prompt.clone(),
        resolved_context: Vec::new(),
        acceptance_criteria: Vec::new(),
        budget,
        session: String::new(),
    });
    TaskCreateRequest {
        goal: request.prompt.clone(),
        agent_id: input.agent_id,
        executor: Some(input.executor),
        executor_target: input.executor_target,
        requested_by: request.requested_by.clone(),
        parent_task_id: None,
        chain_depth: None,
        node_affinity: input.node_affinity,
        spec: Some(spec),
        budget: Some(budget),
        context_refs: None,
        acceptance_criteria: None,
    }
}

impl<S, J> Engine<S, J>
where
    S: TaskStore,
    J: JobQueue,
{
    async fn wait(
        &self,
        id: uuid::Uuid,
        timeout: Duration,
        cancel: CancellationToken,
    ) -> Result<DelegateOutcome, DelegationError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut watch = self.subscribe();
        loop {
            let record = self.load(id).await.map_err(classify)?;
            if terminal(record.status) {
                return Ok(outcome(&record, false));
            }
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(DelegationError::Cancelled),
                () = tokio::time::sleep_until(deadline) => {
                    let record = self.load(id).await.map_err(classify)?;
                    let done = terminal(record.status);
                    return Ok(outcome(&record, !done));
                }
                changed = watch.changed() => {
                    changed.map_err(|error| DelegationError::Failed(Box::new(error)))?;
                }
            }
        }
    }
}

fn outcome(record: &TaskRecord, timed_out: bool) -> DelegateOutcome {
    DelegateOutcome {
        id: record.id,
        status: record.status,
        result: record.result.clone(),
        error: record.error.clone(),
        timed_out,
    }
}

fn classify(error: Error) -> DelegationError {
    match error {
        Error::Cancelled => DelegationError::Cancelled,
        refused @ (Error::ExecutorRequired
        | Error::GoalRequired
        | Error::AgentRequired
        | Error::EmptySession
        | Error::ChainDepthRequired
        | Error::ParentId
        | Error::MissingParent
        | Error::ChainDepth { .. }
        | Error::UnknownTarget { .. }
        | Error::AmbiguousTarget { .. }
        | Error::MeshUnavailable { .. }
        | Error::MeshAgent { .. }
        | Error::MeshTlsRequired
        | Error::GatewayTrustRequired
        | Error::MeshAddress
        | Error::HarnessRequired
        | Error::PathEscape
        | Error::PathMissing
        | Error::FilesRootRequired
        | Error::Unauthorized
        | Error::BaseUrl
        | Error::JobName(_)
        | Error::HeartbeatAgent
        | Error::NodeRequired
        | Error::Budget
        | Error::Pricing
        | Error::SessionSegment
        | Error::ConflictingNode { .. }
        | Error::TimeoutRange
        | Error::Redirect(_)) => DelegationError::Rejected(refused.to_string()),
        failed @ (Error::Connection(_)
        | Error::Operation { .. }
        | Error::QueueAbsent
        | Error::Enqueue { .. }
        | Error::Cleanup { .. }
        | Error::Payload(_)
        | Error::Stored(_)
        | Error::Missing { .. }
        | Error::MissingJob { .. }
        | Error::Conflict { .. }
        | Error::PathIo(_)
        | Error::Blocking(_)
        | Error::Rejected { .. }
        | Error::Response
        | Error::Http(_)
        | Error::MeshTls(_)
        | Error::MeshTlsIo(_)
        | Error::MeshPeer(_)
        | Error::GatewayTls(_)
        | Error::Timeout
        | Error::Clock
        | Error::Catalog(_)) => DelegationError::Failed(Box::new(failed)),
    }
}
