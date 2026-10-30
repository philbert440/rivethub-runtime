mod devices;
mod identities;
mod mesh;
mod models;
mod terminals;

pub use capture::listing::store_roots;

use std::sync::Arc;
use std::time::Duration;

use cli::memory::MemoryExporter;
use config::{Config, Environment};
use gateway::Gateway;
use gateway::sessions::SessionRegistry;
use kernel::agent_presets::AgentPresetStore;
use kernel::filing::FilingStore;
use kernel::hub_state::HubStateStore;
use kernel::memory::MemoryExport;
use kernel::memory::MemoryStore;
use kernel::tidy::TidyStore;
use kernel::tidy::UpkeepStore;
use kernel::topics::DecisionStore;
use kernel::topics::StartupStore;
use kernel::topics::TopicPageStore;
use kernel::topics::TopicStore;
use memory_postgres::PostgresMemory;
use memory_sqlite::SqliteStore;
use thiserror::Error;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum ComposeError {
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error(transparent)]
    Node(#[from] crate::NodeError),
    #[error(transparent)]
    Voice(#[from] gateway::VoiceConfigError),
    #[error(transparent)]
    Settings(#[from] gateway::SettingsError),
    #[error("gateway composition I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("gateway composition worker failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("gateway configuration is invalid: {0}")]
    Invalid(String),
    #[error(transparent)]
    Api(#[from] gateway::error::ApiError),
    #[error(transparent)]
    Terminal(#[from] terminal::Error),
    #[error(transparent)]
    Provider(#[from] crate::models::ModelError),
    #[error(transparent)]
    Agent(#[from] agent_loop::Error),
    #[error(transparent)]
    Hook(#[from] agent_loop::hooks::HookError),
    #[error(transparent)]
    V1(#[from] gateway::routes::v1::Error),
    #[error("gateway configuration JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Mesh(#[from] gateway::mesh::RegistryError),
    #[error(transparent)]
    Channel(#[from] gateway::mesh::ChannelError),
    #[error(transparent)]
    Bind(#[from] gateway::server::BindError),
    #[error(transparent)]
    Devices(#[from] gateway::routes::devices::SetupError),
    #[error(transparent)]
    Pki(#[from] pki::Error),
    #[error(transparent)]
    Tasks(#[from] crate::agent::AgentError),
    #[error(transparent)]
    Memory(#[from] memory_postgres::Error),
}

pub async fn agent_presets(
    memory: &PostgresMemory,
) -> Result<Option<Arc<dyn AgentPresetStore>>, ComposeError> {
    Ok(memory
        .agent_presets()
        .await?
        .map(|store| Arc::new(store) as Arc<dyn AgentPresetStore>))
}

pub async fn hub_state(
    memory: &PostgresMemory,
) -> Result<Option<Arc<dyn HubStateStore>>, ComposeError> {
    let store = memory.hub_state().await?;
    if store.is_none() {
        tracing::warn!(
            "rh_hub_state is missing on the memory database; hub state stays on this node until `rh db migrate` runs"
        );
    }
    Ok(store.map(|store| Arc::new(store) as Arc<dyn HubStateStore>))
}

#[expect(
    missing_debug_implementations,
    reason = "Gateway contains kernel ports without Debug"
)]
pub struct Composition {
    gateway: Arc<Gateway>,
    v1: Option<Arc<gateway::routes::v1::Service>>,
    audit_tasks: JoinSet<Result<(), agent_loop::hooks::HookError>>,
    audit_cancel: CancellationToken,
    tasks: JoinSet<Result<(), ComposeError>>,
    background: tokio_util::task::TaskTracker,
    model_tasks: crate::models::ModelTasks,
    cancel: CancellationToken,
}

fn gateway_settings(config: &Config, env: &Environment) -> Result<gateway::Settings, ComposeError> {
    let node = crate::identity::announce_resolved("gateway", config, env)?;
    Ok(gateway::Settings::from_config_in(
        config,
        env,
        node.hostname,
        node.name,
    )?)
}

#[derive(Default)]
struct Backing<'tasks> {
    presets: Option<Arc<dyn AgentPresetStore>>,
    hub_state: Option<Arc<dyn HubStateStore>>,
    export: Option<Arc<dyn MemoryExport>>,
    topics: Option<(Arc<dyn TopicStore>, Arc<dyn FilingStore>)>,
    decisions: Option<Arc<dyn DecisionStore>>,
    startup: Option<(Arc<dyn TopicPageStore>, Arc<dyn StartupStore>)>,
    tidy: Option<(Arc<dyn TidyStore>, Arc<dyn UpkeepStore>)>,
    tasks: Option<&'tasks crate::agent::LocalTasks>,
}

async fn attach_tasks(
    gateway: Gateway,
    config: &Config,
    env: &Environment,
    shutdown: &CancellationToken,
    tools: Vec<String>,
    local: Option<&crate::agent::LocalTasks>,
) -> Result<crate::agent::Attached, crate::agent::AgentError> {
    match local {
        Some(local) => Ok(crate::agent::Attached {
            gateway: local.mount(gateway, tools),
            watch: None,
        }),
        None => Box::pin(crate::agent::attach(gateway, config, env, shutdown, tools)).await,
    }
}

fn with_backing(gateway: Gateway, backing: &Backing<'_>) -> Gateway {
    let gateway = match backing.presets.clone() {
        Some(presets) => gateway.with_agent_presets(presets),
        None => gateway,
    };
    let gateway = match backing.hub_state.clone() {
        Some(store) => gateway.with_hub_state(store),
        None => gateway,
    };
    let gateway = match backing.export.clone() {
        Some(export) => gateway.with_memory_export(export),
        None => gateway,
    };
    let gateway = match backing.topics.clone() {
        Some((topics, filing)) => gateway.with_topics(topics, filing),
        None => gateway,
    };
    let gateway = match backing.decisions.clone() {
        Some(decisions) => gateway.with_decisions(decisions),
        None => gateway,
    };
    let gateway = match backing.tidy.clone() {
        Some((tidy, upkeep)) => gateway.with_tidy(tidy, upkeep),
        None => gateway,
    };
    match backing.startup.clone() {
        Some((pages, kept)) => gateway.with_startup_memory(pages, kept),
        None => gateway,
    }
}

fn with_session_stores(gateway: Gateway, env: &Environment) -> Gateway {
    let roots = store_roots(env);
    gateway
        .with_transcript_reader(Arc::new(capture::listing::StoreTranscriptReader::new(
            roots.clone(),
        )))
        .with_session_lister(Arc::new(capture::listing::StoreSessionLister::new(roots)))
}

async fn session_registry(
    owners: std::path::PathBuf,
    node_owner: Option<gateway::tenancy::UserId>,
) -> Result<SessionRegistry, ComposeError> {
    let registry = tokio::task::spawn_blocking(move || {
        SessionRegistry::new(Duration::from_secs(300)).with_owners_file(owners)
    })
    .await?;
    Ok(match node_owner {
        Some(owner) => registry.with_node_owner(owner),
        None => registry,
    })
}

impl Composition {
    pub async fn validate_users(config: &Config, env: &Environment) -> Result<(), ComposeError> {
        identities::load(config, env).await?;
        Ok(())
    }

    pub(crate) async fn node_owner(
        config: &Config,
        env: &Environment,
    ) -> Result<gateway::tenancy::UserId, ComposeError> {
        Ok(identities::load(config, env).await?.owner)
    }

    pub async fn failed(&mut self) -> Result<(), ComposeError> {
        tokio::select! {
            result = self.tasks.join_next(), if !self.tasks.is_empty() => {
                if let Some(result) = result { result??; }
                if self.cancel.is_cancelled() {
                    return std::future::pending().await;
                }
                Err(ComposeError::Invalid("mesh task stopped unexpectedly".into()))
            }
            result = self.audit_tasks.join_next(), if !self.audit_tasks.is_empty() => {
                if let Some(result) = result { result??; }
                if self.audit_cancel.is_cancelled() {
                    return std::future::pending().await;
                }
                Err(ComposeError::Invalid("audit task stopped unexpectedly".into()))
            }
            else => std::future::pending().await,
        }
    }
    pub async fn build(
        config: &Config,
        env: &Environment,
        store: Arc<dyn MemoryStore>,
        shutdown: &CancellationToken,
        activities: &kernel::shutdown::Activities,
    ) -> Result<Self, ComposeError> {
        let factory = crate::models::ModelFactory::from_config(config, env, shutdown)
            .await?
            .with_activities(activities.clone());
        Self::with_models(config, env, store, shutdown, &factory).await
    }

    pub async fn on_postgres(
        config: &Config,
        env: &Environment,
        memory: &Arc<PostgresMemory>,
        shutdown: &CancellationToken,
        activities: &kernel::shutdown::Activities,
    ) -> Result<Self, ComposeError> {
        let url = config.memory_postgres_url(env).ok_or_else(|| {
            ComposeError::Invalid("no Postgres URL for the memory database".into())
        })?;
        let backing = Backing {
            presets: agent_presets(memory).await?,
            hub_state: hub_state(memory).await?,
            export: Some(Arc::new(MemoryExporter::postgres(Arc::clone(memory), url))),
            topics: Some((memory.clone(), memory.clone())),
            decisions: Some(memory.clone()),
            startup: Some((memory.clone(), memory.clone())),
            tidy: Some((memory.clone(), memory.clone())),
            tasks: None,
        };
        let factory = crate::models::ModelFactory::from_config(config, env, shutdown)
            .await?
            .with_activities(activities.clone());
        let store: Arc<dyn MemoryStore> = memory.clone();
        Self::compose(config, env, store, backing, shutdown, &factory).await
    }

    pub(crate) async fn on_sqlite(
        config: &Config,
        env: &Environment,
        store: &Arc<SqliteStore>,
        tasks: Option<&crate::agent::LocalTasks>,
        shutdown: &CancellationToken,
        activities: &kernel::shutdown::Activities,
    ) -> Result<Self, ComposeError> {
        let backing = Backing {
            presets: None,
            hub_state: None,
            export: Some(Arc::new(MemoryExporter::sqlite(Arc::clone(store)))),
            topics: Some((store.clone(), store.clone())),
            decisions: Some(store.clone()),
            startup: Some((store.clone(), store.clone())),
            tidy: Some((store.clone(), store.clone())),
            tasks,
        };
        let factory = crate::models::ModelFactory::from_config(config, env, shutdown)
            .await?
            .with_activities(activities.clone());
        let memory: Arc<dyn MemoryStore> = store.clone();
        Self::compose(config, env, memory, backing, shutdown, &factory).await
    }

    pub async fn with_models(
        config: &Config,
        env: &Environment,
        store: Arc<dyn MemoryStore>,
        shutdown: &CancellationToken,
        factory: &crate::models::ModelFactory,
    ) -> Result<Self, ComposeError> {
        Self::compose(config, env, store, Backing::default(), shutdown, factory).await
    }

    async fn compose(
        config: &Config,
        env: &Environment,
        store: Arc<dyn MemoryStore>,
        backing: Backing<'_>,
        shutdown: &CancellationToken,
        factory: &crate::models::ModelFactory,
    ) -> Result<Self, ComposeError> {
        let settings = gateway_settings(config, env)?;
        let node_name = settings.node.clone();
        let section = config.gateway.clone();
        let tls_env = env.clone();
        let shared = config::shared_dir(env);
        let bind = tokio::task::spawn_blocking(move || {
            gateway::server::BindSource::resolve_section(
                section.as_ref(),
                &tls_env,
                &node_name,
                &shared,
            )
        })
        .await?;
        let identity = identities::load(config, env).await?;
        let sessions = Arc::new(
            session_registry(
                gateway::sessions::owners_file(&settings.state_dir),
                identity.tenancy.local_user().cloned(),
            )
            .await?
            .with_shutdown(shutdown.child_token()),
        );
        let mut gateway = Gateway::new(store, settings)
            .with_provider_health(factory.health())
            .with_uploads(crate::service::uploads(env)?)
            .with_tenancy(identity.tenancy.clone())
            .with_identity_required()
            .with_client_certificate_required(bind.require_client)
            .with_sessions(sessions.clone());
        gateway = crate::service::with_voice_and_audio(gateway, config, env)?;
        gateway = with_backing(with_session_stores(gateway, env), &backing);
        gateway = devices::attach(gateway, config, env, &identity.owner, shutdown).await?;
        let background = tokio_util::task::TaskTracker::new();
        if config.terminal_enabled() {
            let owner = identity.tenancy.local_user().cloned().ok_or_else(|| {
                ComposeError::Invalid("a local user is required for terminals".into())
            })?;
            gateway = gateway.with_terminal(
                terminals::build(config, env, sessions, &owner, shutdown, &background).await?,
            );
        }
        let audit_cancel = shutdown.child_token();
        let mut audit_tasks = JoinSet::new();
        let configured =
            models::build(config, env, factory, &audit_cancel, &mut audit_tasks).await?;
        let mut agents = std::collections::BTreeMap::new();
        let mut tool_names = Vec::new();
        let v1 = if let Some(configured) = configured {
            tool_names = configured.tools;
            agents = configured.agents;
            gateway = gateway.with_turn_runner(configured.runner);
            Some(configured.service)
        } else {
            None
        };
        let mut tasks = mesh::start(config, env, agents, identity.tenancy, shutdown).await?;
        let crate::agent::Attached { gateway, watch } =
            attach_tasks(gateway, config, env, shutdown, tool_names, backing.tasks).await?;
        if let Some(watch) = watch {
            tasks.spawn(async move {
                match watch.await {
                    Ok(result) => result.map_err(ComposeError::from),
                    Err(error) => Err(ComposeError::Task(error)),
                }
            });
        }
        Ok(Self {
            gateway: Arc::new(gateway),
            v1,
            audit_tasks,
            audit_cancel,
            tasks,
            background,
            model_tasks: factory.tasks(),
            cancel: shutdown.clone(),
        })
    }

    #[must_use]
    pub fn gateway(&self) -> &Arc<Gateway> {
        &self.gateway
    }

    pub fn router(&self, secure: bool) -> axum::Router {
        match &self.v1 {
            Some(service) => service.router(&self.gateway, secure),
            None => self.gateway.router(secure),
        }
    }

    pub async fn close(mut self) -> Result<(), ComposeError> {
        self.cancel.cancel();
        self.gateway.close().await;
        if let Some(service) = &self.v1 {
            service.shutdown().await;
        }
        self.audit_cancel.cancel();
        while let Some(result) = self.audit_tasks.join_next().await {
            result??;
        }
        while let Some(result) = self.tasks.join_next().await {
            result??;
        }
        self.background.close();
        self.background.wait().await;
        self.model_tasks.close().await;
        Ok(())
    }
}
