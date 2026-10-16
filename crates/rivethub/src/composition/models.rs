use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use config::{Config, Environment};
use kernel::agent::TurnRunner;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use super::ComposeError;

pub(super) struct AgentBinding {
    pub(super) runner: Arc<dyn TurnRunner>,
    pub(super) model: String,
}

pub(super) struct ModelServices {
    pub(super) runner: Arc<dyn TurnRunner>,
    pub(super) service: Arc<gateway::routes::v1::Service>,
    pub(super) agents: BTreeMap<String, AgentBinding>,
    pub(super) tools: Vec<String>,
}

pub(super) async fn build(
    config: &Config,
    env: &Environment,
    factory: &crate::models::ModelFactory,
    cancel: &CancellationToken,
    tasks: &mut JoinSet<Result<(), agent_loop::hooks::HookError>>,
) -> Result<Option<ModelServices>, ComposeError> {
    if config.providers.is_none() {
        return Ok(None);
    }
    let runtime = config
        .runtime
        .as_ref()
        .ok_or_else(|| ComposeError::Invalid("runtime is required for providers".into()))?;
    let default_agent = runtime
        .default_agent
        .as_ref()
        .ok_or_else(|| ComposeError::Invalid("default_agent is required".into()))?;
    let agents = config
        .agents
        .as_ref()
        .ok_or_else(|| ComposeError::Invalid("agents are required for providers".into()))?;
    let workspace = runtime
        .workspace
        .clone()
        .ok_or_else(|| ComposeError::Invalid("runtime.workspace is required".into()))?;
    let state = config::Paths::new(env)?.state_dir();
    let hooks = hooks(workspace, &state, tasks, cancel).await?;
    let mut bindings = BTreeMap::new();
    let mut selected = None;
    let mut tool_names = BTreeSet::new();
    for (id, agent) in agents {
        let provider_id = agent
            .provider
            .as_ref()
            .ok_or_else(|| ComposeError::Invalid(format!("agent {id} requires a provider")))?;
        let crate::models::ModelBinding {
            provider, model, ..
        } = factory.build(config, provider_id, agent.model.as_deref(), cancel)?;
        let built = agent_loop::AgentLoop::new(
            provider_id.clone(),
            provider.clone(),
            Vec::new(),
            hooks.clone(),
        )?;
        tool_names.extend(built.tool_names());
        let runner: Arc<dyn TurnRunner> = Arc::new(built);
        if id == default_agent {
            selected = Some((provider_id.clone(), provider, runner.clone(), model.clone()));
        }
        bindings.insert(id.clone(), AgentBinding { runner, model });
    }
    let (provider_id, provider, runner, model) =
        selected.ok_or_else(|| ComposeError::Invalid("default agent is missing".into()))?;
    let service = gateway::routes::v1::Service::open(
        provider_id,
        provider,
        model,
        &state.join("conversations"),
        cancel.child_token(),
    )
    .await?;
    Ok(Some(ModelServices {
        runner,
        service,
        agents: bindings,
        tools: tool_names.into_iter().collect(),
    }))
}

async fn hooks(
    workspace: std::path::PathBuf,
    state: &std::path::Path,
    tasks: &mut JoinSet<Result<(), agent_loop::hooks::HookError>>,
    cancel: &CancellationToken,
) -> Result<Arc<agent_loop::hooks::HookPipeline>, ComposeError> {
    let fence = agent_loop::fence::WorkspaceFence::new(
        vec![workspace.clone()],
        BTreeSet::new(),
        Some(workspace),
    )
    .await?;
    tokio::fs::create_dir_all(state).await?;
    let shell = agent_loop::shell::ShellDanger::new(BTreeSet::new())?;
    let audit =
        agent_loop::audit::AuditLog::open(&state.join("audit.jsonl"), tasks, cancel.child_token())
            .await?;
    Ok(Arc::new(agent_loop::hooks::HookPipeline::new(
        shell, fence, audit,
    )))
}
