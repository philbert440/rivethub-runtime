use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use config::Config;
use config::Environment;
use config::POSTGRES_URL_VARIABLE;
use gateway::server::BindSource;
use memory_postgres::PostgresTasks;
use thiserror::Error;
use wire::gateway::agents::AgentPreset;
use wire::gateway::agents::AgentsFile;
use wire::mesh::MeshNode;

use super::LocalBinding;
use super::TaskEngine;
use super::ports::Roster;
use crate::service::StartupError;
use crate::service::postgres_url;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("SQLite gateway composition requires a complete SQLite MemoryStore adapter")]
    SqliteUnavailable,
    #[error(
        "no Postgres URL for the memory database — set \"memory.postgres.connection_string\" or \"{POSTGRES_URL_VARIABLE}\""
    )]
    Url,
    #[error(transparent)]
    Settings(#[from] gateway::SettingsError),
    #[error(transparent)]
    Tasks(#[from] tasks::Error),
    #[error(transparent)]
    Store(#[from] kernel::tasks::TaskError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("task configuration JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Mesh(#[from] gateway::mesh::RegistryError),
    #[error("task worker failed: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("tasks are disabled")]
    Disabled,
    #[error("gateway token is empty")]
    EmptyToken,
    #[error("agent preset {0} has an empty node")]
    EmptyNode(String),
    #[error("agent preset name {0} is duplicated")]
    DuplicatePreset(String),
    #[error("agent preset id {0} is duplicated")]
    DuplicatePresetId(String),
    #[error("pricing for {0} needs input_per_mtok and output_per_mtok")]
    Pricing(String),
    #[error("gateway url is invalid")]
    GatewayUrl,
    #[error(
        "gateway certificate has no loopback name (127.0.0.1, ::1, or localhost); reissue the node certificate with a loopback subject alternative name"
    )]
    GatewayName,
    #[error(
        "gateway is bound only to {host}; bind the gateway host to 127.0.0.1, ::1, localhost, 0.0.0.0, or :: so the agent can dial it as the loopback owner"
    )]
    GatewayBind { host: String },
    #[error("gateway certificate is invalid")]
    GatewayCertificate,
    #[error("agents.json escapes the state directory")]
    Escape,
    #[error("{0}")]
    Invalid(String),
    #[error("task watch closed")]
    WatchClosed,
    #[error(transparent)]
    Node(#[from] crate::NodeError),
}

impl AgentError {
    pub(crate) fn exit(&self) -> crate::Exit {
        match self {
            Self::SqliteUnavailable
            | Self::Url
            | Self::Store(kernel::tasks::TaskError::Unavailable(_)) => crate::Exit::Memory,
            Self::Io(_) => crate::Exit::Serve,
            Self::Settings(_)
            | Self::Tasks(_)
            | Self::Store(_)
            | Self::Json(_)
            | Self::Mesh(_)
            | Self::Join(_)
            | Self::Disabled
            | Self::EmptyToken
            | Self::EmptyNode(_)
            | Self::DuplicatePreset(_)
            | Self::DuplicatePresetId(_)
            | Self::Pricing(_)
            | Self::GatewayUrl
            | Self::GatewayName
            | Self::GatewayBind { .. }
            | Self::GatewayCertificate
            | Self::Escape
            | Self::Invalid(_)
            | Self::WatchClosed
            | Self::Node(_) => crate::Exit::Failure,
        }
    }
}

pub(super) struct Plan {
    pub(super) node: String,
    worker: String,
    postgres: String,
    pub gateway: url::Url,
    files_root: Option<PathBuf>,
    pub allowed: Option<Vec<wire::harness::HarnessId>>,
    pricing: BTreeMap<String, tasks::TokenRate>,
    pub api_token: Option<String>,
    pub mesh_identity: Option<tasks::MeshIdentity>,
    pub gateway_trust: Option<tasks::MeshIdentity>,
    pub mesh: bool,
    state_dir: PathBuf,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "Debug omits the postgres url, gateway token, and mesh client key"
    )
)]
impl std::fmt::Debug for Plan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Plan")
            .field("node", &self.node)
            .field("worker", &self.worker)
            .field("gateway", &self.gateway)
            .field("files_root", &self.files_root)
            .field("allowed", &self.allowed)
            .field("pricing_models", &self.pricing.len())
            .field("has_postgres_url", &!self.postgres.is_empty())
            .field("has_api_token", &self.api_token.is_some())
            .field("has_mesh_identity", &self.mesh_identity.is_some())
            .field("has_gateway_trust", &self.gateway_trust.is_some())
            .field("mesh", &self.mesh)
            .field("state_dir", &self.state_dir)
            .finish()
    }
}

impl Plan {
    pub(super) fn settings(&self) -> tasks::Settings {
        tasks::Settings {
            node: self.node.clone(),
            worker: self.worker.clone(),
            files_root: self.files_root.clone(),
            pricing: self.pricing.clone(),
            api_token: self.api_token.clone(),
        }
    }
}

pub(super) struct Connected {
    pub tasks: PostgresTasks,
    pub roster: Arc<Roster>,
    pub plan: Plan,
}

impl Connected {
    pub(super) fn engine(
        self,
        activities: &kernel::shutdown::Activities,
    ) -> Result<Arc<TaskEngine>, AgentError> {
        let directory = self.roster.directory(&self.plan)?;
        Ok(Arc::new(
            tasks::Engine::new(
                self.tasks.clone(),
                self.tasks,
                directory,
                self.plan.settings(),
            )?
            .with_activities(activities.clone()),
        ))
    }
}

pub(super) fn ensure_enabled(config: &Config) -> Result<(), AgentError> {
    if config
        .tasks
        .as_ref()
        .is_some_and(|tasks| tasks.enabled == Some(false))
    {
        Err(AgentError::Disabled)
    } else {
        Ok(())
    }
}

pub(super) async fn plan(config: &Config, env: &Environment) -> Result<Plan, AgentError> {
    let identity = crate::identity::resolve(config, env)?;
    if identity.name.is_empty() {
        return Err(AgentError::Invalid("node name is empty".into()));
    }
    let node = identity.name;
    let worker = format!("{node}:agent");
    let settings = gateway::Settings::from_config_in(config, env, identity.hostname, node.clone())?;
    let endpoint = super::dial::endpoint(config, env, &node).await?;
    Ok(Plan {
        node,
        worker,
        postgres: require_postgres(config, env)?,
        gateway: endpoint.url,
        files_root: settings.files_root,
        allowed: settings.allowed_harnesses,
        pricing: pricing(config)?,
        api_token: api_token(config)?,
        mesh_identity: None,
        gateway_trust: endpoint.trust,
        mesh: mesh_enabled(config),
        state_dir: settings.state_dir,
    })
}

pub(super) async fn standalone(
    config: &Config,
    env: &Environment,
    gateway: url::Url,
) -> Result<(tasks::Directory, tasks::Settings), AgentError> {
    let identity = crate::identity::resolve(config, env)?;
    if identity.name.is_empty() {
        return Err(AgentError::Invalid("node name is empty".into()));
    }
    let node = identity.name;
    let worker = format!("{node}:agent");
    let gateway_settings =
        gateway::Settings::from_config_in(config, env, identity.hostname, node.clone())?;
    let state_dir = gateway_settings.state_dir.clone();
    let presets = load_presets(state_dir).await?;
    let mut directory = tasks::Directory::new(node.clone(), gateway)?;
    install_local(&mut directory, config)?;
    for preset in presets {
        directory.preset(preset);
    }
    Ok((
        directory,
        tasks::Settings {
            node,
            worker,
            files_root: gateway_settings.files_root,
            pricing: pricing(config)?,
            api_token: api_token(config)?,
        },
    ))
}

fn install_local(directory: &mut tasks::Directory, config: &Config) -> Result<(), AgentError> {
    for (name, binding) in local_agents(config)? {
        directory.local_agent(
            name,
            tasks::LocalAgent {
                model: binding.model,
                provider: binding.provider,
                effort: None,
                harness: None,
                system_prompt: None,
                working_dir: None,
            },
        );
    }
    Ok(())
}

pub(super) async fn connect(config: &Config, env: &Environment) -> Result<Connected, AgentError> {
    let mut plan = plan(config, env).await?;
    plan.mesh_identity = load_mesh_identity(config, env).await?;
    let presets = load_presets(plan.state_dir.clone()).await?;
    let nodes = load_nodes(config, env).await?;
    let roster = Arc::new(Roster {
        node: plan.node.clone(),
        mesh: plan.mesh,
        local: local_agents(config)?,
        presets,
        nodes,
    });
    let tasks = PostgresTasks::connect(&plan.postgres).await?;
    Ok(Connected {
        tasks,
        roster,
        plan,
    })
}

fn require_postgres(config: &Config, env: &Environment) -> Result<String, AgentError> {
    match postgres_url(config, env) {
        Ok(url) => Ok(url),
        Err(StartupError::Url) => Err(AgentError::Url),
        Err(StartupError::SqliteUnavailable) => Err(AgentError::SqliteUnavailable),
        Err(error) => Err(AgentError::Invalid(error.to_string())),
    }
}

fn pricing(config: &Config) -> Result<BTreeMap<String, tasks::TokenRate>, AgentError> {
    let Some(section) = &config.tasks else {
        return Ok(BTreeMap::new());
    };
    let Some(entries) = &section.pricing else {
        return Ok(BTreeMap::new());
    };
    let mut rates = BTreeMap::new();
    for (model, entry) in entries {
        let Some(input) = entry.input_per_mtok else {
            return Err(AgentError::Pricing(model.clone()));
        };
        let Some(output) = entry.output_per_mtok else {
            return Err(AgentError::Pricing(model.clone()));
        };
        rates.insert(model.clone(), tasks::TokenRate::new(input, output)?);
    }
    Ok(rates)
}

fn api_token(config: &Config) -> Result<Option<String>, AgentError> {
    match config
        .gateway
        .as_ref()
        .and_then(|gateway| gateway.token.clone())
    {
        Some(token) if token.is_empty() => Err(AgentError::EmptyToken),
        Some(token) => Ok(Some(token)),
        None => Ok(None),
    }
}

fn mesh_enabled(config: &Config) -> bool {
    config
        .mesh
        .as_ref()
        .is_some_and(|mesh| mesh.enabled == Some(true))
}

struct MeshTlsFiles {
    cert: PathBuf,
    key: PathBuf,
    ca: PathBuf,
}

enum MeshChoice {
    Gateway,
    Explicit(config::model::MeshTlsPaths),
}

fn mesh_choice(config: &Config) -> Result<Option<MeshChoice>, AgentError> {
    let Some(mesh) = config.mesh.as_ref() else {
        return Ok(None);
    };
    if mesh.enabled != Some(true) {
        return Ok(None);
    }
    match &mesh.tls {
        config::model::MeshTls::Enabled(true) => Ok(Some(MeshChoice::Gateway)),
        config::model::MeshTls::Enabled(false) => Err(tasks::Error::MeshTlsRequired.into()),
        config::model::MeshTls::Paths(paths) => Ok(Some(MeshChoice::Explicit(paths.clone()))),
    }
}

fn gateway_mesh_files(
    section: Option<&config::model::GatewaySection>,
    env: &Environment,
    node_name: &str,
    shared: &std::path::Path,
) -> Result<MeshTlsFiles, AgentError> {
    let source = BindSource::resolve_section(section, env, node_name, shared);
    Ok(MeshTlsFiles {
        cert: require_path(source.tls_cert)?,
        key: require_path(source.tls_key)?,
        ca: require_path(source.tls_ca)?,
    })
}

fn explicit_mesh_files(
    paths: &config::model::MeshTlsPaths,
    node_name: &str,
    shared: &std::path::Path,
) -> MeshTlsFiles {
    let resolved = paths.resolve(node_name, shared);
    MeshTlsFiles {
        cert: resolved.cert,
        key: resolved.key,
        ca: resolved.ca,
    }
}

fn require_path(path: Option<PathBuf>) -> Result<PathBuf, AgentError> {
    path.filter(|item| !item.as_os_str().is_empty())
        .ok_or_else(|| tasks::Error::MeshTlsRequired.into())
}

async fn load_mesh_identity(
    config: &Config,
    env: &Environment,
) -> Result<Option<tasks::MeshIdentity>, AgentError> {
    let Some(choice) = mesh_choice(config)? else {
        return Ok(None);
    };
    let node = crate::identity::resolve(config, env)?;
    let section = config.gateway.clone();
    let env = env.clone();
    let shared = config::shared_dir(&env);
    match tokio::task::spawn_blocking(move || {
        let files = match choice {
            MeshChoice::Gateway => gateway_mesh_files(section.as_ref(), &env, &node.name, &shared)?,
            MeshChoice::Explicit(paths) => explicit_mesh_files(&paths, &node.name, &shared),
        };
        tasks::MeshIdentity::from_files(&files.cert, &files.key, &files.ca)
            .map_err(AgentError::from)
    })
    .await
    {
        Ok(result) => Ok(Some(result?)),
        Err(error) => Err(AgentError::Join(error)),
    }
}

fn local_agents(config: &Config) -> Result<BTreeMap<String, LocalBinding>, AgentError> {
    let Some(agents) = &config.agents else {
        return Ok(BTreeMap::new());
    };
    let mut local = BTreeMap::new();
    for (name, entry) in agents {
        if name.is_empty() {
            return Err(AgentError::Invalid("agent id is empty".into()));
        }
        local.insert(
            name.clone(),
            LocalBinding {
                model: entry.model.clone(),
                provider: entry.provider.clone(),
            },
        );
    }
    Ok(local)
}

async fn load_presets(state_dir: PathBuf) -> Result<Vec<AgentPreset>, AgentError> {
    match tokio::task::spawn_blocking(move || read_presets(&state_dir)).await {
        Ok(result) => result,
        Err(error) => Err(AgentError::Join(error)),
    }
}

fn read_presets(state_dir: &Path) -> Result<Vec<AgentPreset>, AgentError> {
    let root = match state_dir.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let file = root.join("agents.json");
    if !file.starts_with(&root) {
        return Err(AgentError::Escape);
    }
    match std::fs::symlink_metadata(&file) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err(AgentError::Escape),
        Ok(metadata) if !metadata.is_file() => {
            return Err(AgentError::Invalid("agents.json is not a file".into()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    }
    let canonical = file.canonicalize()?;
    if !canonical.starts_with(&root) {
        return Err(AgentError::Escape);
    }
    let parsed: AgentsFile = serde_json::from_slice(&std::fs::read(canonical)?)?;
    validate_presets(&parsed.agents)?;
    Ok(parsed.agents)
}

fn validate_presets(presets: &[AgentPreset]) -> Result<(), AgentError> {
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for preset in presets {
        if preset.name.is_empty() {
            return Err(AgentError::Invalid("agent preset name is empty".into()));
        }
        if preset.id.is_empty() {
            return Err(AgentError::Invalid("agent preset id is empty".into()));
        }
        if preset.node.as_deref() == Some("") {
            return Err(AgentError::EmptyNode(preset.name.clone()));
        }
        if !names.insert(preset.name.clone()) {
            return Err(AgentError::DuplicatePreset(preset.name.clone()));
        }
        if !ids.insert(preset.id.clone()) {
            return Err(AgentError::DuplicatePresetId(preset.id.clone()));
        }
    }
    Ok(())
}

async fn load_nodes(config: &Config, env: &Environment) -> Result<Vec<MeshNode>, AgentError> {
    if !mesh_enabled(config) {
        return Ok(Vec::new());
    }
    let registry =
        gateway::mesh::Registry::open(config::shared_dir(env), PathBuf::from("mesh.json")).await?;
    match registry.snapshot().await? {
        Some(file) => Ok(file.nodes.into_values().collect()),
        None => Ok(Vec::new()),
    }
}
