use std::sync::Arc;

use config::provider::ProviderBody;
use config::{Config, Environment};
use kernel::memory::MemoryStore as _;
use kernel::memory::{EmbeddingStore, SummaryStore};
use kernel::model::ModelProvider;
use memory_postgres::PostgresMemory;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use workers::{
    EMBED_CONCURRENCY, EmbedSettings, ProcessRole, Runner, RunnerConfig, SummarySettings,
    WorkerDeps,
};

use crate::Exit;
use crate::service::{StartupError, load, postgres, postgres_url, report};

#[derive(Debug, Error)]
enum RootError {
    #[error(transparent)]
    Startup(#[from] StartupError),
    #[error(transparent)]
    Worker(#[from] workers::WorkerError),
    #[error(transparent)]
    Provider(#[from] providers::BuildError),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error("{0}")]
    Invalid(String),
}

impl RootError {
    fn exit(&self) -> Exit {
        match self {
            Self::Startup(error) => error.exit(),
            Self::Worker(_) | Self::Provider(_) | Self::Join(_) | Self::Invalid(_) => Exit::Failure,
        }
    }
}

pub async fn start(env: &Environment, role: ProcessRole, check: bool) -> Exit {
    if check {
        return finish(check_role(env, role).await, "worker configuration valid");
    }
    let signals = match crate::signals::install() {
        Ok(signals) => signals,
        Err(exit) => return exit,
    };
    let result = run(env, role, signals.token()).await;
    if let Err(exit) = signals.finish().await {
        return exit;
    }
    finish(result, "")
}

fn finish(result: Result<(), RootError>, ready: &str) -> Exit {
    match result {
        Ok(()) => {
            if !ready.is_empty() {
                tracing::info!("{ready}");
            }
            Exit::Ok
        }
        Err(error) => {
            report_root(&error);
            error.exit()
        }
    }
}

fn report_root(error: &RootError) {
    match error {
        RootError::Startup(error) => report(error),
        RootError::Worker(_)
        | RootError::Provider(_)
        | RootError::Join(_)
        | RootError::Invalid(_) => tracing::error!("{error}"),
    }
}

async fn check_role(env: &Environment, role: ProcessRole) -> Result<(), RootError> {
    role.claim_roles()?;
    let owned = env.clone();
    let config = tokio::task::spawn_blocking(move || load(&owned)).await??;
    postgres_url(&config, env)?;
    let model = provider_model(&config)?;
    EmbedSettings::from_config(&config)?;
    SummarySettings::standard(model)?;
    Ok(())
}

async fn run(
    env: &Environment,
    role: ProcessRole,
    shutdown: CancellationToken,
) -> Result<(), RootError> {
    let _guard = shutdown.clone().drop_guard();
    role.claim_roles()?;
    let owned = env.clone();
    let config = tokio::task::spawn_blocking(move || load(&owned)).await??;
    let memory = Arc::new(postgres(&config, env).await?);
    let result = async {
        let queue = memory.queue().await.map_err(StartupError::from)?;
        let model = provider_model(&config)?;
        let runner = build_runner(role, Arc::clone(&memory), queue, &config, model)?;
        tracing::info!("worker running {role}");
        drive(runner, shutdown.clone()).await
    }
    .await;
    shutdown.cancel();
    let closed = memory.close().await.map_err(StartupError::from);
    closed?;
    result?;
    tracing::info!("worker stopped");
    Ok(())
}

fn build_runner(
    role: ProcessRole,
    memory: Arc<PostgresMemory>,
    queue: memory_postgres::PostgresQueue,
    config: &Config,
    model: String,
) -> Result<Runner<memory_postgres::PostgresQueue>, RootError> {
    let concrete = open_provider(config)?;
    let provider: Arc<dyn ModelProvider> = Arc::new(concrete);
    let targets: Arc<dyn EmbeddingStore> = memory.clone();
    let summaries: Arc<dyn SummaryStore> = memory.clone();
    let wiki: Arc<dyn kernel::wiki::builder::WikiBuilderStore> = memory.clone();
    let store: Arc<dyn kernel::memory::MemoryStore> = memory;
    let deps = WorkerDeps {
        targets,
        summaries,
        provider,
        embed: EmbedSettings::from_config(config)?,
        summary: SummarySettings::standard(model.clone())?,
        memory: store,
        tags: workers::TagSettings::new(model.clone(), false)?,
    };
    Ok(Runner::new(
        Arc::new(queue),
        RunnerConfig::new(role, EMBED_CONCURRENCY)?,
        deps,
    )
    .with_wiki_builder(workers::wiki_builder::WikiBuilderDeps { store: wiki, model }))
}

async fn drive(
    runner: Runner<memory_postgres::PostgresQueue>,
    shutdown: CancellationToken,
) -> Result<(), RootError> {
    let tracker = TaskTracker::new();
    let token = shutdown.child_token();
    let task = tracker.spawn(async move { runner.run(token).await });
    tokio::pin!(task);
    let joined = tokio::select! {
        biased;
        () = shutdown.cancelled() => (&mut task).await,
        result = &mut task => result,
    };
    tracker.close();
    tracker.wait().await;
    joined??;
    Ok(())
}

fn provider_model(config: &Config) -> Result<String, RootError> {
    let concrete = open_provider(config)?;
    let model = agent_model(config, concrete.model())?;
    if model.is_empty() {
        return Err(RootError::Invalid("summary model is empty".into()));
    }
    Ok(model)
}

fn agent_model(config: &Config, provider_model: &str) -> Result<String, RootError> {
    let agent = default_agent(config)?;
    match &agent.model {
        Some(model) => Ok(model.clone()),
        None => Ok(provider_model.to_owned()),
    }
}

fn open_provider(config: &Config) -> Result<providers::OpenAiCompatible, RootError> {
    let id = default_agent(config)?
        .provider
        .as_ref()
        .ok_or_else(|| RootError::Invalid("the default agent requires a provider".into()))?;
    let providers = config
        .providers
        .as_ref()
        .ok_or_else(|| RootError::Invalid("providers are required".into()))?;
    let slot = providers
        .get(id)
        .ok_or_else(|| RootError::Invalid(format!("provider {id} is missing")))?;
    let ProviderBody::Openai(body) = &slot.body else {
        return Err(RootError::Invalid(format!(
            "provider {id} must be OpenAI compatible"
        )));
    };
    let kind = slot
        .kind
        .ok_or_else(|| RootError::Invalid("provider type is required".into()))?;
    Ok(providers::OpenAiCompatible::build(kind, body)?)
}

fn default_agent(config: &Config) -> Result<&config::model::AgentEntry, RootError> {
    let runtime = config
        .runtime
        .as_ref()
        .ok_or_else(|| RootError::Invalid("runtime is required for providers".into()))?;
    let name = runtime
        .default_agent
        .as_ref()
        .ok_or_else(|| RootError::Invalid("default_agent is required".into()))?;
    let agents = config
        .agents
        .as_ref()
        .ok_or_else(|| RootError::Invalid("agents are required for providers".into()))?;
    agents
        .get(name)
        .ok_or_else(|| RootError::Invalid(format!("default agent {name} is missing")))
}
