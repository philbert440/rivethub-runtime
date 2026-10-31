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

pub(super) async fn build(
    config: &Config,
    env: &Environment,
    factory: &crate::models::ModelFactory,
    cancel: &CancellationToken,
    tasks: &mut JoinSet<Result<(), agent_loop::hooks::HookError>>,
) -> Result<BTreeMap<String, AgentBinding>, ComposeError> {
    if config.providers.is_none() {
        return Ok(BTreeMap::new());
    }
    let runtime = config
        .runtime
        .as_ref()
        .ok_or_else(|| ComposeError::Invalid("runtime is required for providers".into()))?;
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
    for (id, agent) in agents {
        let provider_id = agent
            .provider
            .as_ref()
            .ok_or_else(|| ComposeError::Invalid(format!("agent {id} requires a provider")))?;
        let crate::models::ModelBinding {
            provider, model, ..
        } = factory.build(config, provider_id, agent.model.as_deref(), cancel)?;
        let built =
            agent_loop::AgentLoop::new(provider_id.clone(), provider, Vec::new(), hooks.clone())?;
        let runner: Arc<dyn TurnRunner> = Arc::new(built);
        bindings.insert(id.clone(), AgentBinding { runner, model });
    }
    Ok(bindings)
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
