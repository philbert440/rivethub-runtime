use std::sync::Arc;

use config::{Config, Environment};
use kernel::memory::{EmbeddingStore, MemoryStore, SummaryStore};
use kernel::queue::JobQueue;
use kernel::wiki::builder::WikiBuilderStore;
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
pub enum RootError {
    #[error(transparent)]
    Startup(#[from] StartupError),
    #[error(transparent)]
    Worker(#[from] workers::WorkerError),
    #[error(transparent)]
    Provider(#[from] crate::models::ModelError),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error("{0}")]
    Invalid(String),
}

impl RootError {
    pub(crate) fn exit(&self) -> Exit {
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

pub(crate) fn report_root(error: &RootError) {
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
    let shutdown = CancellationToken::new();
    let factory = model_factory(&config, env).await?;
    let binding = model_binding(&config, &factory, &shutdown)?;
    check_embeddings(role, &binding)?;
    let model = binding.model;
    EmbedSettings::from_config(&config)?;
    SummarySettings::standard(model)?;
    crate::identity::announce_resolved("worker", &config, env).map_err(StartupError::from)?;
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
    crate::identity::announce_resolved("worker", &config, env).map_err(StartupError::from)?;
    let memory = Arc::new(postgres(&config, env).await?);
    let result = async {
        let queue = memory.queue().await.map_err(StartupError::from)?;
        let targets: Arc<dyn EmbeddingStore> = memory.clone();
        let summaries: Arc<dyn SummaryStore> = memory.clone();
        let wiki: Arc<dyn WikiBuilderStore> = memory.clone();
        let store: Arc<dyn MemoryStore> = memory.clone();
        let runner = build_runner(
            role,
            WorkerPorts {
                queue: Arc::new(queue),
                targets,
                summaries,
                memory: store,
                wiki,
            },
            &config,
            env,
            &shutdown,
        )
        .await?;
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

#[expect(
    missing_debug_implementations,
    reason = "worker dependencies are kernel ports without Debug"
)]
pub struct WorkerPorts<Q> {
    pub queue: Arc<Q>,
    pub targets: Arc<dyn EmbeddingStore>,
    pub summaries: Arc<dyn SummaryStore>,
    pub memory: Arc<dyn MemoryStore>,
    pub wiki: Arc<dyn WikiBuilderStore>,
}

pub async fn build_runner<Q: JobQueue + 'static>(
    role: ProcessRole,
    ports: WorkerPorts<Q>,
    config: &Config,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<ComposedRunner<Q>, RootError> {
    let factory = model_factory(config, env).await?;
    with_models(role, ports, config, &factory, shutdown)
}

pub fn with_models<Q: JobQueue + 'static>(
    role: ProcessRole,
    ports: WorkerPorts<Q>,
    config: &Config,
    factory: &crate::models::ModelFactory,
    shutdown: &CancellationToken,
) -> Result<ComposedRunner<Q>, RootError> {
    role.claim_roles()?;
    let cancel = shutdown.child_token();
    let binding = model_binding(config, factory, &cancel)?;
    check_embeddings(role, &binding)?;
    let crate::models::ModelBinding {
        provider, model, ..
    } = binding;
    let deps = WorkerDeps {
        targets: ports.targets,
        summaries: ports.summaries,
        provider,
        embed: EmbedSettings::from_config(config)?,
        summary: SummarySettings::standard(model.clone())?,
        memory: ports.memory,
        tags: workers::TagSettings::new(model.clone(), false)?,
    };
    let runner = Runner::new(
        ports.queue,
        RunnerConfig::new(role, EMBED_CONCURRENCY)?,
        deps,
    )
    .with_wiki_builder(workers::wiki_builder::WikiBuilderDeps {
        store: ports.wiki,
        model,
    });
    Ok(ComposedRunner {
        runner,
        tasks: factory.tasks(),
        cancel,
    })
}

#[expect(
    missing_debug_implementations,
    reason = "worker runner holds kernel ports without Debug"
)]
pub struct ComposedRunner<Q> {
    pub runner: Runner<Q>,
    tasks: TaskTracker,
    cancel: CancellationToken,
}

impl<Q: JobQueue + 'static> ComposedRunner<Q> {
    pub async fn run(&self, shutdown: CancellationToken) -> Result<(), workers::WorkerError> {
        let result = self.runner.run(shutdown.clone()).await;
        self.cancel.cancel();
        self.tasks.close();
        self.tasks.wait().await;
        result
    }
}

async fn drive<Q: JobQueue + 'static>(
    runner: ComposedRunner<Q>,
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

fn check_embeddings(
    role: ProcessRole,
    binding: &crate::models::ModelBinding,
) -> Result<(), RootError> {
    if matches!(role, ProcessRole::Embedder | ProcessRole::All) && !binding.embeddings {
        return Err(
            crate::models::ModelError::Cli(providers::cli::CliError::MissingEmbedder).into(),
        );
    }
    Ok(())
}

async fn model_factory(
    config: &Config,
    env: &Environment,
) -> Result<crate::models::ModelFactory, RootError> {
    Ok(crate::models::ModelFactory::for_provider(config, env, provider_id(config)?).await?)
}

fn model_binding(
    config: &Config,
    factory: &crate::models::ModelFactory,
    shutdown: &CancellationToken,
) -> Result<crate::models::ModelBinding, RootError> {
    Ok(factory.build(
        config,
        provider_id(config)?,
        default_agent(config)?.model.as_deref(),
        shutdown,
    )?)
}

fn provider_id(config: &Config) -> Result<&str, RootError> {
    default_agent(config)?
        .provider
        .as_deref()
        .ok_or_else(|| RootError::Invalid("the default agent requires a provider".into()))
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
