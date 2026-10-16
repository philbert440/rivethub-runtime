mod import;

use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::worker::ComposedRunner;
use config::{Config, Environment, ServiceSwitch};
use gateway::server::ServeOptions;
use kernel::memory::{EmbeddingStore, MemoryStore, SummaryStore};
use kernel::queue::{FixedJob, Job, JobKeyMode, JobName, JobQueue};
use kernel::shutdown::Activities;
use kernel::tasks::Delegation;
use kernel::wiki::builder::WikiBuilderStore;
use memory_sqlite::SqliteStore;
use thiserror::Error;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use workers::ProcessRole;

use crate::Exit;
use crate::service::{StartupError, listen, report};
use crate::sidecar::{self, SidecarError};
use crate::worker::{self, RootError, WorkerPorts};

#[derive(Debug, Error)]
enum LocalError {
    #[error(transparent)]
    Import(#[from] import::Error),
    #[error(transparent)]
    Startup(#[from] StartupError),
    #[error(transparent)]
    Worker(#[from] RootError),
    #[error(transparent)]
    Mcp(#[from] SidecarError),
    #[error(transparent)]
    Store(#[from] memory_sqlite::SqliteError),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Tasks(#[from] tasks::Error),
    #[error(transparent)]
    Agent(#[from] crate::agent::AgentError),
    #[error(transparent)]
    Queue(#[from] kernel::queue::JobQueueError),
    #[error("gateway address is not a loopback url")]
    GatewayUrl,
}

impl LocalError {
    fn exit(&self) -> Exit {
        match self {
            Self::Startup(error) => error.exit(),
            Self::Worker(error) => error.exit(),
            Self::Mcp(error) => error.exit(),
            Self::Import(_)
            | Self::Store(_)
            | Self::Join(_)
            | Self::Tasks(_)
            | Self::Agent(_)
            | Self::Queue(_)
            | Self::GatewayUrl => Exit::Failure,
        }
    }
}

pub async fn start(env: &Environment, prepared: cli::Prepared) -> Exit {
    let cli::Prepared {
        config,
        claim,
        database,
    } = prepared;
    let mut held = HeldClaim { claim: Some(claim) };
    let signals = match crate::signals::install() {
        Ok(signals) => signals,
        Err(exit) => return exit,
    };
    let activities = Activities::new();
    let token = signals.token();
    let run_token = token.clone();
    let result = crate::shutdown::bounded(
        &token,
        &activities,
        Box::pin(run(env, &config, database, run_token, activities.clone())),
    )
    .await;
    let finished = signals.finish().await;
    let released = held.release();
    match result {
        Ok(inner) => finish(inner, finished, released),
        Err(running) => {
            tracing::debug!("bounded shutdown: {running}");
            if let Err(error) = released {
                tracing::error!("pid file removal failed: {error}");
            }
            match finished {
                Ok(()) => Exit::Failure,
                Err(exit) => exit,
            }
        }
    }
}

fn finish(
    result: Result<(), LocalError>,
    finished: Result<(), Exit>,
    released: Result<(), config::StateError>,
) -> Exit {
    match (result, finished, released) {
        (Ok(()), Ok(()), Ok(())) => Exit::Ok,
        (Err(error), _, released) => {
            if let Err(release) = released {
                tracing::error!("pid file removal failed: {release}");
            }
            report_local(&error);
            error.exit()
        }
        (Ok(()), Err(exit), released) => {
            if let Err(release) = released {
                tracing::error!("pid file removal failed: {release}");
            }
            exit
        }
        (Ok(()), Ok(()), Err(error)) => {
            tracing::error!("pid file removal failed: {error}");
            Exit::Failure
        }
    }
}

fn report_local(error: &LocalError) {
    match error {
        LocalError::Startup(error) => report(error),
        LocalError::Worker(error) => worker::report_root(error),
        LocalError::Mcp(error) => sidecar::report_error(error),
        LocalError::Import(_)
        | LocalError::Store(_)
        | LocalError::Join(_)
        | LocalError::Tasks(_)
        | LocalError::Agent(_)
        | LocalError::Queue(_)
        | LocalError::GatewayUrl => {
            tracing::error!("{error}");
        }
    }
}

struct GatewayNode {
    composition: crate::composition::Composition,
    listener: tokio::net::TcpListener,
    tls: Option<gateway::tls::TlsPaths>,
    runners: Vec<(ProcessRole, ComposedRunner<SqliteStore>)>,
    activities: Activities,
}

async fn run(
    env: &Environment,
    config: &Config,
    database: PathBuf,
    shutdown: CancellationToken,
    activities: Activities,
) -> Result<(), LocalError> {
    let _guard = shutdown.clone().drop_guard();
    let store = open_store(database).await?;
    import::pending(&store, config, &shutdown).await?;
    let switches = config.service_switches(env);
    let node = bind_node(&store, &switches, config, env, &shutdown, &activities).await?;
    supervise(node, &switches, config, env, store, shutdown).await
}

async fn bind_node(
    store: &Arc<SqliteStore>,
    switches: &[ServiceSwitch],
    config: &Config,
    env: &Environment,
    shutdown: &CancellationToken,
    activities: &Activities,
) -> Result<GatewayNode, LocalError> {
    let runners = build_runners(
        store,
        worker_roles(switches),
        config,
        env,
        shutdown,
        activities,
    )
    .await?;
    let composition = crate::composition::Composition::build(
        config,
        env,
        Arc::clone(store) as Arc<dyn MemoryStore>,
        shutdown,
        activities,
    )
    .await
    .map_err(StartupError::from)?;
    let (listener, tls) = match listen(config, env).await {
        Ok(bound) => bound,
        Err(error) => {
            if let Err(close) = composition.close().await {
                tracing::error!("composition close failed: {close}");
            }
            return Err(error.into());
        }
    };
    Ok(GatewayNode {
        composition,
        listener,
        tls,
        runners,
        activities: activities.clone(),
    })
}

async fn supervise(
    node: GatewayNode,
    switches: &[ServiceSwitch],
    config: &Config,
    env: &Environment,
    store: Arc<SqliteStore>,
    shutdown: CancellationToken,
) -> Result<(), LocalError> {
    let GatewayNode {
        composition,
        listener,
        tls,
        runners,
        activities,
    } = node;
    let addr = listener.local_addr().map_err(StartupError::from)?;
    let secure = tls.is_some();
    tracing::info!(
        "gateway listening on {addr}{}",
        if secure { " (tls)" } else { " (plain)" }
    );
    let tracker = TaskTracker::new();
    let (failed_tx, mut failed_rx) = mpsc::unbounded_channel();
    if let Err(error) = (NodeBoot {
        tracker: &tracker,
        failed: &failed_tx,
        switches,
        config,
        env,
        store: &store,
        shutdown: &shutdown,
        activities: &activities,
    })
    .start(runners, loopback_url(addr, secure)?)
    .await
    {
        shutdown.cancel();
        note_close(composition.close().await.map_err(StartupError::from));
        drop(failed_tx);
        tracker.close();
        tracker.wait().await;
        return Err(error);
    }
    let serving = gateway::server::serve_router_with(
        listener,
        composition.router(secure),
        tls,
        shutdown.clone(),
        ServeOptions::default(),
        activities.clone(),
    );
    tokio::pin!(serving);
    let outcome = await_node(composition, &mut failed_rx, &mut serving, &shutdown).await;
    drop(failed_tx);
    tracker.close();
    tracker.wait().await;
    outcome?;
    tracing::info!("gateway stopped");
    Ok(())
}

struct NodeBoot<'a> {
    tracker: &'a TaskTracker,
    failed: &'a mpsc::UnboundedSender<LocalError>,
    switches: &'a [ServiceSwitch],
    config: &'a Config,
    env: &'a Environment,
    store: &'a Arc<SqliteStore>,
    shutdown: &'a CancellationToken,
    activities: &'a Activities,
}

impl NodeBoot<'_> {
    async fn start(
        &self,
        runners: Vec<(ProcessRole, ComposedRunner<SqliteStore>)>,
        gateway: url::Url,
    ) -> Result<(), LocalError> {
        if self.switches.contains(&ServiceSwitch::Wiki) {
            enqueue_backfill(self.store).await?;
        }
        spawn_workers(self.tracker, self.failed, self.shutdown, runners);
        let delegation = if self.switches.contains(&ServiceSwitch::Mcp) {
            self.delegation(gateway).await?
        } else {
            None
        };
        self.spawn_mcp(delegation)
    }

    async fn delegation(
        &self,
        gateway: url::Url,
    ) -> Result<Option<Arc<dyn Delegation>>, LocalError> {
        if self
            .config
            .tasks
            .as_ref()
            .is_some_and(|tasks| tasks.enabled == Some(false))
        {
            return Ok(None);
        }
        let (directory, settings) =
            crate::agent::standalone_tasks(self.config, self.env, gateway).await?;
        let engine = Arc::new(
            tasks::Engine::new(
                self.store.as_ref().clone(),
                self.store.as_ref().clone(),
                directory,
                settings,
            )?
            .with_activities(self.activities.clone()),
        );
        let port: Arc<dyn Delegation> = engine.clone();
        let failed = self.failed.clone();
        let token = self.shutdown.child_token();
        self.tracker.spawn(async move {
            if let Err(error) = engine.serve(token).await {
                deliver(&failed, LocalError::Tasks(error));
            }
        });
        Ok(Some(port))
    }

    fn spawn_mcp(&self, delegation: Option<Arc<dyn Delegation>>) -> Result<(), LocalError> {
        if !self.switches.contains(&ServiceSwitch::Mcp) {
            return Ok(());
        }
        let task = sidecar::memory_future(
            self.config,
            Arc::clone(self.store) as Arc<dyn MemoryStore>,
            delegation,
            sidecar::display_zone(self.env),
            self.shutdown.clone(),
        )?;
        let failed = self.failed.clone();
        self.tracker.spawn(async move {
            if let Err(error) = task.await {
                deliver(&failed, LocalError::Mcp(error));
            }
        });
        Ok(())
    }
}

async fn enqueue_backfill(store: &SqliteStore) -> Result<(), LocalError> {
    store
        .enqueue(Job {
            name: JobName::Fixed(FixedJob::EnqueueWikiBackfill),
            payload: wire::JsonObject::default(),
            key: Some("wiki-backfill".to_owned()),
            key_mode: JobKeyMode::Replace,
            priority: 5,
            run_at: None,
            max_attempts: None,
        })
        .await?;
    Ok(())
}

fn loopback_url(addr: SocketAddr, secure: bool) -> Result<url::Url, LocalError> {
    let ip = if addr.ip().is_unspecified() {
        IpAddr::V4(Ipv4Addr::LOCALHOST)
    } else {
        addr.ip()
    };
    let mut url = url::Url::parse("http://127.0.0.1/").map_err(|_| LocalError::GatewayUrl)?;
    if secure {
        url.set_scheme("https")
            .map_err(|()| LocalError::GatewayUrl)?;
    }
    url.set_ip_host(ip).map_err(|()| LocalError::GatewayUrl)?;
    url.set_port(Some(addr.port()))
        .map_err(|()| LocalError::GatewayUrl)?;
    url.set_path("/");
    Ok(url)
}

async fn await_node(
    mut composition: crate::composition::Composition,
    failed_rx: &mut mpsc::UnboundedReceiver<LocalError>,
    serving: &mut std::pin::Pin<
        &mut impl std::future::Future<Output = Result<(), gateway::server::ServeError>>,
    >,
    shutdown: &CancellationToken,
) -> Result<(), LocalError> {
    tokio::select! {
        biased;
        () = shutdown.cancelled() => {
            composition.close().await.map_err(StartupError::from)?;
            serving.await.map_err(StartupError::from).map_err(LocalError::from)
        }
        Some(error) = failed_rx.recv() => {
            shutdown.cancel();
            note_close(composition.close().await.map_err(StartupError::from));
            note_serve(serving.await.map_err(StartupError::from));
            Err(error)
        }
        result = composition.failed() => {
            shutdown.cancel();
            composition.close().await.map_err(StartupError::from)?;
            result.map_err(StartupError::from)?;
            serving.await.map_err(StartupError::from).map_err(LocalError::from)
        }
        result = &mut *serving => {
            shutdown.cancel();
            composition.close().await.map_err(StartupError::from)?;
            result.map_err(StartupError::from).map_err(LocalError::from)
        }
    }
}

fn note_close(result: Result<(), StartupError>) {
    if let Err(error) = result {
        tracing::error!("composition close failed: {error}");
    }
}

fn note_serve(result: Result<(), StartupError>) {
    if let Err(error) = result {
        tracing::error!("gateway serve ended during shutdown: {error}");
    }
}

async fn open_store(path: PathBuf) -> Result<Arc<SqliteStore>, LocalError> {
    let store = tokio::task::spawn_blocking(move || SqliteStore::open(path)).await??;
    Ok(Arc::new(store))
}

async fn build_runners(
    store: &Arc<SqliteStore>,
    roles: Vec<ProcessRole>,
    config: &Config,
    env: &Environment,
    shutdown: &CancellationToken,
    activities: &Activities,
) -> Result<Vec<(ProcessRole, ComposedRunner<SqliteStore>)>, LocalError> {
    let mut built = Vec::new();
    for role in roles {
        let ports = WorkerPorts {
            queue: Arc::clone(store),
            targets: Arc::clone(store) as Arc<dyn EmbeddingStore>,
            summaries: Arc::clone(store) as Arc<dyn SummaryStore>,
            memory: Arc::clone(store) as Arc<dyn MemoryStore>,
            wiki: Arc::clone(store) as Arc<dyn WikiBuilderStore>,
        };
        built.push((
            role,
            worker::build_runner(role, ports, config, env, shutdown, activities).await?,
        ));
    }
    Ok(built)
}

fn worker_roles(switches: &[ServiceSwitch]) -> Vec<ProcessRole> {
    let mut roles = Vec::new();
    for switch in switches {
        match switch {
            ServiceSwitch::Workers => {
                roles.push(ProcessRole::Embedder);
                roles.push(ProcessRole::Summarizer);
            }
            ServiceSwitch::Tagging => roles.push(ProcessRole::Categorizer),
            ServiceSwitch::Wiki => roles.push(ProcessRole::WikiBuilder),
            ServiceSwitch::Gateway | ServiceSwitch::Agent | ServiceSwitch::Mcp => {}
        }
    }
    roles
}

fn spawn_workers(
    tracker: &TaskTracker,
    failed: &mpsc::UnboundedSender<LocalError>,
    shutdown: &CancellationToken,
    runners: Vec<(ProcessRole, ComposedRunner<SqliteStore>)>,
) {
    for (role, runner) in runners {
        let failed = failed.clone();
        let token = shutdown.clone();
        tracker.spawn(async move {
            tracing::info!("worker running {role}");
            if let Err(error) = runner.run(token).await {
                deliver(&failed, LocalError::Worker(RootError::Worker(error)));
            }
        });
    }
}

fn deliver(failed: &mpsc::UnboundedSender<LocalError>, error: LocalError) {
    match failed.send(error) {
        Ok(()) => {}
        Err(send) => tracing::error!("component failure was not delivered: {}", send.0),
    }
}

struct HeldClaim {
    claim: Option<config::NodeClaim>,
}

impl HeldClaim {
    fn release(&mut self) -> Result<(), config::StateError> {
        match self.claim.take() {
            Some(claim) => claim.release(),
            None => Ok(()),
        }
    }
}

impl Drop for HeldClaim {
    fn drop(&mut self) {
        if let Some(claim) = self.claim.take() {
            match claim.release() {
                Ok(()) => {}
                Err(error) => tracing::error!("pid file removal failed: {error}"),
            }
        }
    }
}
