mod assemble;
mod dial;
mod ports;

use std::sync::Arc;

use config::Config;
use config::Environment;
use gateway::Gateway;
use kernel::tasks::Delegation;
use kernel::tasks::TaskWatch;
use memory_postgres::PostgresTasks;
use thiserror::Error;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::Exit;
use crate::service::StartupError;
use crate::service::load;
use crate::service::report;
pub use assemble::AgentError;

#[derive(Debug)]
pub(super) struct LocalBinding {
    model: Option<String>,
    provider: Option<String>,
}

pub(super) type TaskEngine = tasks::Engine<PostgresTasks, PostgresTasks>;

pub(crate) async fn standalone_tasks(
    config: &Config,
    env: &Environment,
    gateway: url::Url,
) -> Result<(tasks::Directory, tasks::Settings), AgentError> {
    assemble::standalone(config, env, gateway).await
}

#[derive(Debug, Error)]
enum RootError {
    #[error(transparent)]
    Startup(#[from] StartupError),
    #[error(transparent)]
    Agent(#[from] AgentError),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
}

impl RootError {
    fn exit(&self) -> Exit {
        match self {
            Self::Startup(error) => error.exit(),
            Self::Agent(error) => error.exit(),
            Self::Join(_) => Exit::Failure,
        }
    }
}

pub(crate) struct Attached {
    pub gateway: Gateway,
    pub watch: Option<JoinHandle<Result<(), AgentError>>>,
}

pub(crate) struct DelegationRun {
    pub port: Arc<dyn Delegation>,
    cancel: CancellationToken,
    watch: JoinHandle<Result<(), AgentError>>,
}

impl DelegationRun {
    pub(crate) async fn shutdown(self) -> Result<(), AgentError> {
        self.cancel.cancel();
        match self.watch.await {
            Ok(result) => result,
            Err(error) => Err(AgentError::Join(error)),
        }
    }
}

pub async fn start(env: &Environment, check: bool) -> Exit {
    if check {
        return finish(validate(env).await, "agent configuration valid");
    }
    let env = env.clone();
    crate::shutdown::signals_then(
        async move |shutdown, activities| run(&env, shutdown, &activities).await,
        |result| finish(result, ""),
    )
    .await
}

pub(crate) async fn attach(
    gateway: Gateway,
    config: &Config,
    env: &Environment,
    shutdown: &CancellationToken,
    tools: Vec<String>,
) -> Result<Attached, AgentError> {
    match crate::service::postgres_url(config, env) {
        Ok(_) => {}
        Err(
            crate::service::StartupError::Url | crate::service::StartupError::SqliteUnavailable,
        ) => {
            return Ok(Attached {
                gateway,
                watch: None,
            });
        }
        Err(error) => return Err(AgentError::Invalid(error.to_string())),
    }
    let connected = assemble::connect(config, env).await?;
    let directory = connected.roster.directory(&connected.plan)?;
    let harness = gateway.terminal().and_then(|terminal| terminal.adapter());
    let catalog = Arc::new(tasks::Catalog::new(
        &directory,
        connected.plan.mesh,
        tools,
        harness,
    ));
    let api = ports::api(
        Arc::clone(&connected.roster),
        catalog,
        connected.plan.allowed.clone(),
        &connected.tasks,
    );
    let watch = watch_store(connected.tasks, shutdown.child_token()).await?;
    Ok(Attached {
        gateway: gateway.with_tasks(api),
        watch: Some(watch),
    })
}

pub(crate) async fn open_delegation(
    config: &Config,
    env: &Environment,
    shutdown: &CancellationToken,
    activities: &kernel::shutdown::Activities,
) -> Result<DelegationRun, AgentError> {
    let connected = assemble::connect(config, env).await?;
    let tasks = connected.tasks.clone();
    let engine = connected.engine(activities)?;
    let cancel = shutdown.child_token();
    let cloned = Arc::clone(&engine);
    let port: Arc<dyn Delegation> = cloned;
    let watch = tokio::spawn(bridge(tasks, engine, cancel.clone()));
    Ok(DelegationRun {
        port,
        cancel,
        watch,
    })
}

async fn watch_store(
    tasks: PostgresTasks,
    cancel: CancellationToken,
) -> Result<JoinHandle<Result<(), AgentError>>, AgentError> {
    let handle = tasks.spawn_watch(cancel).await?;
    Ok(tokio::spawn(async move { join_watch(handle).await }))
}

async fn bridge(
    tasks: PostgresTasks,
    engine: Arc<TaskEngine>,
    cancel: CancellationToken,
) -> Result<(), AgentError> {
    let mut events = TaskWatch::subscribe(&tasks);
    let mut drive = tasks.spawn_watch(cancel.child_token()).await?;
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            changed = events.changed() => {
                changed.map_err(|_| AgentError::WatchClosed)?;
                engine.wake_waiters();
            }
            finished = &mut drive => {
                return match finished {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(error)) => Err(AgentError::Store(error)),
                    Err(error) => Err(AgentError::Join(error)),
                };
            }
        }
    }
    join_watch(drive).await
}

async fn join_watch(
    handle: impl std::future::Future<
        Output = Result<Result<(), kernel::tasks::TaskError>, tokio::task::JoinError>,
    >,
) -> Result<(), AgentError> {
    match handle.await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(AgentError::Store(error)),
        Err(error) => Err(AgentError::Join(error)),
    }
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
        RootError::Agent(_) | RootError::Join(_) => tracing::error!("{error}"),
    }
}

async fn validate(env: &Environment) -> Result<(), RootError> {
    let config = read_config(env).await?;
    assemble::ensure_enabled(&config)?;
    crate::identity::announce_resolved("agent", &config, env).map_err(AgentError::from)?;
    drop(assemble::plan(&config, env).await?);
    Ok(())
}

async fn run(
    env: &Environment,
    shutdown: CancellationToken,
    activities: &kernel::shutdown::Activities,
) -> Result<(), RootError> {
    let _guard = shutdown.clone().drop_guard();
    let config = read_config(env).await?;
    assemble::ensure_enabled(&config)?;
    crate::identity::announce_resolved("agent", &config, env).map_err(AgentError::from)?;
    let connected = assemble::connect(&config, env).await?;
    let engine = connected.engine(activities)?;
    tracing::info!("agent running");
    engine.serve(shutdown).await.map_err(AgentError::from)?;
    tracing::info!("agent stopped");
    Ok(())
}

async fn read_config(env: &Environment) -> Result<Config, RootError> {
    let owned = env.clone();
    Ok(tokio::task::spawn_blocking(move || load(&owned)).await??)
}
