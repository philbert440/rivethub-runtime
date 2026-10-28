pub mod auth;
pub mod error;
pub mod extract;
mod history;
pub mod mesh;
pub mod routes;
pub mod server;
pub mod sessions;
pub mod tenancy;
pub mod term;
pub mod tls;
mod tls_resolve;
pub mod uploads;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::middleware;
use axum::routing;
use kernel::agent::TurnRunner;
use kernel::memory::MemoryStore;
use tower::ServiceBuilder;
use tower::util::MapRequestLayer;
use wire::harness::HarnessId;

use crate::auth::DenialLog;
use crate::routes::audio::AudioBridge;
use crate::routes::audio::VoiceProxy;

pub use crate::routes::audio::AudioConfig;
pub use crate::routes::audio::VoiceConfigError;
pub use crate::routes::audio::VoiceSettings;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REVISION: Option<&str> = option_env!("RIVETHUB_BUILD_REVISION");

const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);
const DEFAULT_MESH_CACHE_TTL: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error("{variable}={text} is not a millisecond count")]
    MeshCache {
        variable: &'static str,
        text: String,
        #[source]
        source: std::num::ParseIntError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub hostname: String,
    pub node: String,
    pub allowed_origins: Vec<String>,
    pub allowed_hosts: Vec<String>,
    pub files_root: Option<PathBuf>,
    pub static_dir: Option<PathBuf>,
    pub root_redirect: Option<String>,
    pub state_dir: PathBuf,
    pub directory_root: PathBuf,
    pub mesh_file: Option<PathBuf>,
    pub shared_dir: PathBuf,
    pub legacy_mesh_dir: PathBuf,
    pub allowed_harnesses: Option<Vec<HarnessId>>,
    pub devices_enabled: bool,
    pub probe_timeout: Duration,
    pub mesh_ca: Option<PathBuf>,
    pub mesh_cache_ttl: Duration,
}

#[derive(Clone)]
pub(crate) struct GuardState {
    pub(crate) gateway: Arc<Gateway>,
    pub(crate) secure: bool,
}

impl Settings {
    pub fn from_config_in(
        config: &config::Config,
        env: &config::Environment,
        hostname: String,
        node: String,
    ) -> Result<Self, SettingsError> {
        let section = config.gateway.clone().unwrap_or_default();
        let paths = config::Paths::new(env)?;
        let state_dir = canonical_dir(
            match configured_path(env, "RIVETHUB_STATE_DIR", "RIVETOS_DEN_STATE_DIR") {
                Some(path) => path,
                None => paths.state_dir(),
            },
        );
        let shared_dir = canonical_dir(config::shared_dir(env));
        let mesh_ca = tls_resolve::mesh_ca(Some(&section), env, &shared_dir);
        Ok(Self {
            hostname,
            node,
            allowed_origins: section.allowed_origins,
            allowed_hosts: section.allowed_hosts,
            files_root: configured_files_root(section.files_root),
            static_dir: match configured_path(env, "RIVETHUB_STATIC_DIR", "RIVETOS_DEN_STATIC_DIR")
            {
                Some(path) => Some(path),
                None => section.static_dir,
            },
            root_redirect: match configured_text(
                env,
                "RIVETHUB_ROOT_REDIRECT",
                "RIVETOS_DEN_ROOT_REDIRECT",
            ) {
                Some(target) => Some(target),
                None => section.root_redirect,
            },
            directory_root: state_dir.join("agents"),
            state_dir,
            mesh_file: configured_path(env, "RIVETHUB_MESH_FILE", "RIVETOS_DEN_MESH_FILE"),
            shared_dir,
            legacy_mesh_dir: paths.legacy_home(),
            allowed_harnesses: section
                .allowed_harnesses
                .map(|entries| entries.iter().filter_map(known_harness).collect()),
            devices_enabled: section.devices.enabled == Some(true),
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
            mesh_ca,
            mesh_cache_ttl: mesh_cache_ttl(env)?,
        })
    }
}

fn known_harness(slot: &config::HarnessSlot) -> Option<HarnessId> {
    match slot {
        config::HarnessSlot::Known(id) => Some(*id),
        config::HarnessSlot::Unknown(_) => None,
    }
}

fn canonical_dir(path: PathBuf) -> PathBuf {
    let mut missing = Vec::new();
    let mut cursor = path.clone();
    loop {
        match std::fs::canonicalize(&cursor) {
            Ok(mut found) => {
                for name in missing.into_iter().rev() {
                    found.push(name);
                }
                return found;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = cursor.file_name().map(std::ffi::OsStr::to_os_string) else {
                    return path;
                };
                missing.push(name);
                if !cursor.pop() {
                    return path;
                }
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "agent path could not be canonicalized"
                );
                return path;
            }
        }
    }
}

fn configured_text(env: &config::Environment, current: &str, legacy: &str) -> Option<String> {
    if let Some(notice) = env.legacy_notice(current, legacy) {
        tracing::warn!("{notice}");
    }
    let text = match env.text(current) {
        Some(value) => Some(value),
        None => env.text(legacy),
    };
    text.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn configured_path(env: &config::Environment, current: &str, legacy: &str) -> Option<PathBuf> {
    configured_text(env, current, legacy).map(PathBuf::from)
}

fn mesh_cache_ttl(env: &config::Environment) -> Result<Duration, SettingsError> {
    if let Some(notice) = env.legacy_notice("RIVETHUB_MESH_CACHE_MS", "RIVETOS_DEN_MESH_CACHE_MS") {
        tracing::warn!("{notice}");
    }
    if let Some(text) = nonempty(env.text("RIVETHUB_MESH_CACHE_MS")) {
        return parse_cache_millis("RIVETHUB_MESH_CACHE_MS", text);
    }
    if let Some(text) = nonempty(env.text("RIVETOS_DEN_MESH_CACHE_MS")) {
        return parse_cache_millis("RIVETOS_DEN_MESH_CACHE_MS", text);
    }
    Ok(DEFAULT_MESH_CACHE_TTL)
}

fn nonempty(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|value| !value.is_empty())
}

fn parse_cache_millis(variable: &'static str, text: &str) -> Result<Duration, SettingsError> {
    match text.parse::<u64>() {
        Ok(millis) => Ok(Duration::from_millis(millis)),
        Err(source) => Err(SettingsError::MeshCache {
            variable,
            text: text.to_owned(),
            source,
        }),
    }
}

fn configured_files_root(root: Option<PathBuf>) -> Option<PathBuf> {
    match root {
        Some(path) if !path.as_os_str().is_empty() => Some(path),
        Some(_) | None => None,
    }
}

#[expect(
    missing_debug_implementations,
    reason = "the store port is a trait object without Debug, so the derive does not compile and a hand-written impl trips the manualderive rule"
)]
pub struct Gateway {
    store: Arc<dyn MemoryStore>,
    turn_runner: Option<Arc<dyn TurnRunner>>,
    settings: Settings,
    embed_provider: bool,
    policy: auth::OriginPolicy,
    denials: DenialLog,
    tenancy: tenancy::Tenancy,
    identity_required: bool,
    client_certificate_required: bool,
    sessions: Arc<sessions::SessionRegistry>,
    terminal: Option<Arc<term::TerminalService>>,
    transcript: Option<Arc<dyn kernel::transcript::TranscriptReader>>,
    session_lister: Option<Arc<dyn kernel::transcript::SessionLister>>,
    history: history::History,
    uploads: Option<uploads::UploadService>,
    presets: routes::agents::AgentRegistry,
    hub_state: Arc<dyn kernel::hub_state::HubStateStore>,
    certificate_authority: Option<Arc<dyn kernel::ca::CertificateAuthority>>,
    devices: routes::devices::DeviceRuntime,
    mesh_cache: routes::MeshCache,
    audio: Option<AudioBridge>,
    voice: VoiceProxy,
    tasks: Option<Arc<routes::tasks::TasksApi>>,
    providers: Vec<kernel::model::ProviderHealth>,
    memory_export: Option<Arc<dyn kernel::memory::MemoryExport>>,
    topics: Option<Arc<dyn kernel::topics::TopicStore>>,
    startup: Option<StartupMemory>,
}

struct StartupMemory {
    pages: Arc<dyn kernel::topics::TopicPageStore>,
    kept: Arc<dyn kernel::topics::StartupStore>,
}

impl Gateway {
    #[must_use]
    pub fn new(store: Arc<dyn MemoryStore>, settings: Settings) -> Self {
        let policy = auth::OriginPolicy::new(
            settings.allowed_origins.clone(),
            settings.allowed_hosts.clone(),
        );
        let presets = routes::agents::open(&settings);
        let hub_state = routes::hub_state::open(&settings);
        Self {
            store,
            turn_runner: None,
            settings,
            embed_provider: false,
            policy,
            denials: DenialLog::default(),
            tenancy: tenancy::Tenancy::default(),
            identity_required: false,
            client_certificate_required: true,
            sessions: Arc::new(sessions::SessionRegistry::new(
                std::time::Duration::from_secs(300),
            )),
            terminal: None,
            transcript: None,
            session_lister: None,
            history: history::History::default(),
            uploads: None,
            presets,
            hub_state,
            certificate_authority: None,
            devices: routes::devices::DeviceRuntime::unconfigured(),
            mesh_cache: routes::MeshCache::default(),
            audio: None,
            voice: VoiceProxy::unconfigured(),
            tasks: None,
            providers: Vec::new(),
            memory_export: None,
            topics: None,
            startup: None,
        }
    }

    #[must_use]
    pub fn with_agent_presets(
        mut self,
        store: Arc<dyn kernel::agent_presets::AgentPresetStore>,
    ) -> Self {
        self.presets = routes::agents::backed_by(&self.settings, store);
        self
    }

    #[must_use]
    pub fn with_hub_state(mut self, store: Arc<dyn kernel::hub_state::HubStateStore>) -> Self {
        self.hub_state = store;
        self
    }

    pub(crate) fn hub_state(&self) -> &dyn kernel::hub_state::HubStateStore {
        self.hub_state.as_ref()
    }

    #[must_use]
    pub fn with_memory_export(mut self, export: Arc<dyn kernel::memory::MemoryExport>) -> Self {
        self.memory_export = Some(export);
        self
    }

    #[must_use]
    pub fn with_topics(mut self, store: Arc<dyn kernel::topics::TopicStore>) -> Self {
        self.topics = Some(store);
        self
    }

    pub(crate) fn topics(&self) -> Result<&Arc<dyn kernel::topics::TopicStore>, error::ApiError> {
        self.topics
            .as_ref()
            .ok_or_else(|| error::ApiError::Unavailable("topic store unavailable".into()))
    }

    #[must_use]
    pub fn with_startup_memory(
        mut self,
        pages: Arc<dyn kernel::topics::TopicPageStore>,
        kept: Arc<dyn kernel::topics::StartupStore>,
    ) -> Self {
        self.startup = Some(StartupMemory { pages, kept });
        self
    }

    pub(crate) fn startup_memory(
        &self,
    ) -> Option<(
        &dyn kernel::topics::TopicPageStore,
        &dyn kernel::topics::StartupStore,
    )> {
        self.startup
            .as_ref()
            .map(|startup| (startup.pages.as_ref(), startup.kept.as_ref()))
    }

    pub(crate) fn memory_export(&self) -> Option<&Arc<dyn kernel::memory::MemoryExport>> {
        self.memory_export.as_ref()
    }

    #[must_use]
    pub fn with_provider_health(mut self, providers: Vec<kernel::model::ProviderHealth>) -> Self {
        self.providers = providers;
        self
    }

    #[must_use]
    pub fn with_tasks(mut self, api: Arc<routes::tasks::TasksApi>) -> Self {
        self.tasks = Some(api);
        self
    }

    #[must_use]
    pub fn with_turn_runner(mut self, runner: Arc<dyn TurnRunner>) -> Self {
        self.turn_runner = Some(runner);
        self
    }

    #[must_use]
    pub fn with_devices(mut self, devices: routes::devices::DeviceRuntime) -> Self {
        self.devices = devices;
        self
    }

    #[must_use]
    pub fn with_certificate_authority(
        mut self,
        authority: Arc<dyn kernel::ca::CertificateAuthority>,
    ) -> Self {
        self.certificate_authority = Some(Arc::clone(&authority));
        self.devices = self.devices.with_authority(authority);
        self
    }

    pub(crate) fn certificate_authority(
        &self,
    ) -> Option<&Arc<dyn kernel::ca::CertificateAuthority>> {
        self.certificate_authority.as_ref()
    }

    #[must_use]
    pub fn with_embed_provider(mut self) -> Self {
        self.embed_provider = true;
        self
    }

    #[must_use]
    pub fn with_identity_required(mut self) -> Self {
        self.identity_required = true;
        self
    }

    #[must_use]
    pub fn with_client_certificate_required(mut self, required: bool) -> Self {
        self.client_certificate_required = required;
        self
    }

    #[must_use]
    pub(crate) fn client_certificate_required(&self) -> bool {
        self.client_certificate_required
    }

    #[must_use]
    pub fn with_tenancy(mut self, tenancy: tenancy::Tenancy) -> Self {
        self.tenancy = tenancy;
        self
    }

    #[must_use]
    pub fn with_sessions(mut self, sessions: Arc<sessions::SessionRegistry>) -> Self {
        self.sessions = sessions;
        self
    }

    #[must_use]
    pub fn with_terminal(mut self, terminal: Arc<term::TerminalService>) -> Self {
        self.sessions = terminal.registry.clone();
        self.terminal = Some(terminal);
        self
    }

    #[must_use]
    pub fn with_audio(mut self, config: AudioConfig) -> Self {
        self.audio = Some(AudioBridge::start(config));
        self
    }

    pub fn with_voice(mut self, settings: VoiceSettings) -> Result<Self, VoiceConfigError> {
        self.voice = VoiceProxy::from_settings(settings)?;
        Ok(self)
    }

    pub fn sessions(&self) -> &Arc<sessions::SessionRegistry> {
        &self.sessions
    }

    #[must_use]
    pub fn terminal(&self) -> Option<&Arc<term::TerminalService>> {
        self.terminal.as_ref()
    }

    #[must_use]
    pub fn with_uploads(mut self, uploads: uploads::UploadService) -> Self {
        self.uploads = Some(uploads);
        self
    }

    pub(crate) fn tasks(&self) -> Option<&Arc<routes::tasks::TasksApi>> {
        self.tasks.as_ref()
    }

    pub(crate) fn uploads(&self) -> Option<&uploads::UploadService> {
        self.uploads.as_ref()
    }

    #[must_use]
    pub fn with_transcript_reader(
        mut self,
        reader: Arc<dyn kernel::transcript::TranscriptReader>,
    ) -> Self {
        self.transcript = Some(reader);
        self
    }

    #[must_use]
    pub fn with_session_lister(
        mut self,
        lister: Arc<dyn kernel::transcript::SessionLister>,
    ) -> Self {
        self.session_lister = Some(lister);
        self
    }

    #[must_use]
    pub fn session_lister(&self) -> Option<&Arc<dyn kernel::transcript::SessionLister>> {
        self.session_lister.as_ref()
    }

    pub(crate) fn history(&self) -> &history::History {
        &self.history
    }

    pub async fn close(&self) {
        if let Some(audio) = &self.audio {
            audio.shutdown().await;
        }
        if let Some(terminal) = &self.terminal {
            terminal.close().await;
        }
        if let Some(uploads) = &self.uploads {
            uploads.close().await;
        }
        self.sessions.close().await;
    }

    pub(crate) fn audio(&self) -> Option<&AudioBridge> {
        self.audio.as_ref()
    }

    pub(crate) fn voice(&self) -> &VoiceProxy {
        &self.voice
    }

    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub(crate) fn provider_health(&self) -> &[kernel::model::ProviderHealth] {
        &self.providers
    }

    pub(crate) fn store(&self) -> &Arc<dyn MemoryStore> {
        &self.store
    }

    #[must_use]
    pub fn embeds_wired(&self) -> bool {
        self.embed_provider
    }

    pub(crate) fn policy(&self) -> &auth::OriginPolicy {
        &self.policy
    }

    pub(crate) fn denials(&self) -> &DenialLog {
        &self.denials
    }

    pub(crate) fn presets(&self) -> &routes::agents::AgentRegistry {
        &self.presets
    }

    pub(crate) fn devices(&self) -> &routes::devices::DeviceRuntime {
        &self.devices
    }

    pub(crate) fn mesh_cache(&self) -> &routes::MeshCache {
        &self.mesh_cache
    }

    pub fn router(self: &Arc<Self>, secure: bool) -> Router {
        let guard_state = GuardState {
            gateway: Arc::clone(self),
            secure,
        };
        let routed = Router::new()
            .route("/healthz", routing::any(routes::healthz))
            .route(
                "/api/capture",
                routing::post(routes::capture::ingest)
                    .fallback(routes::method_not_allowed)
                    .layer(axum::extract::DefaultBodyLimit::max(
                        routes::capture_schema::body_limit(),
                    )),
            )
            .nest("/api/memory", routes::memory::router())
            .merge(routes::sessions::router())
            .merge(routes::terminal::router())
            .merge(routes::router());
        let mounted = routes::mount(routed).with_state(Arc::clone(self));
        let rewritten = ServiceBuilder::new()
            .layer(MapRequestLayer::new(routes::rewrite_request))
            .service(mounted);
        let mut served = Router::new();
        if let Some(api) = &self.tasks {
            served = served.merge(routes::tasks::router(Arc::clone(api)));
        }
        served
            .fallback_service(rewritten)
            .layer(middleware::from_fn_with_state(guard_state, auth::guard))
    }
}
