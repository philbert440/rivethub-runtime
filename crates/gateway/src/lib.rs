pub mod auth;
pub mod error;
pub mod extract;
pub mod mesh;
pub mod routes;
pub mod server;
pub mod sessions;
pub mod tenancy;
pub mod term;
pub mod tls;
pub mod uploads;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::middleware;
use axum::routing;
use kernel::agent::TurnRunner;
use kernel::memory::MemoryStore;
use wire::harness::HarnessId;

use crate::auth::DenialLog;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub hostname: String,
    pub node: String,
    pub allowed_origins: Vec<String>,
    pub allowed_hosts: Vec<String>,
    pub files_root: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub directory_root: PathBuf,
    pub shared_dir: PathBuf,
    pub allowed_harnesses: Option<Vec<HarnessId>>,
}

#[derive(Clone)]
pub(crate) struct GuardState {
    pub(crate) gateway: Arc<Gateway>,
    pub(crate) secure: bool,
}

impl Settings {
    #[must_use]
    pub fn from_config_in(
        config: &config::Config,
        env: &config::Environment,
        hostname: String,
        node: String,
    ) -> Self {
        let section = config.gateway.clone().unwrap_or_default();
        let state_dir = canonical_dir(config::Paths::new(env).state_dir());
        Self {
            hostname,
            node,
            allowed_origins: section.allowed_origins,
            allowed_hosts: section.allowed_hosts,
            files_root: configured_files_root(section.files_root),
            directory_root: state_dir.join("agents"),
            state_dir,
            shared_dir: canonical_dir(config::shared_dir(env)),
            allowed_harnesses: section
                .allowed_harnesses
                .map(|entries| entries.iter().filter_map(known_harness).collect()),
        }
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
    sessions: Arc<sessions::SessionRegistry>,
    terminal: Option<Arc<term::TerminalService>>,
    transcript: Option<Arc<dyn kernel::transcript::TranscriptReader>>,
    uploads: Option<uploads::UploadService>,
    presets: routes::agents::AgentRegistry,
    devices: routes::devices::DeviceRuntime,
}

impl Gateway {
    #[must_use]
    pub fn new(store: Arc<dyn MemoryStore>, settings: Settings) -> Self {
        let policy = auth::OriginPolicy::new(
            settings.allowed_origins.clone(),
            settings.allowed_hosts.clone(),
        );
        let presets = routes::agents::open(&settings);
        Self {
            store,
            turn_runner: None,
            settings,
            embed_provider: false,
            policy,
            denials: DenialLog::default(),
            tenancy: tenancy::Tenancy::default(),
            sessions: Arc::new(sessions::SessionRegistry::new(
                std::time::Duration::from_secs(300),
            )),
            terminal: None,
            transcript: None,
            uploads: None,
            presets,
            devices: routes::devices::DeviceRuntime::unconfigured(),
        }
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
        self.devices = self.devices.with_authority(authority);
        self
    }

    #[must_use]
    pub fn with_embed_provider(mut self) -> Self {
        self.embed_provider = true;
        self
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

    pub fn sessions(&self) -> &Arc<sessions::SessionRegistry> {
        &self.sessions
    }

    #[must_use]
    pub fn with_uploads(mut self, uploads: uploads::UploadService) -> Self {
        self.uploads = Some(uploads);
        self
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

    pub async fn close(&self) {
        if let Some(terminal) = &self.terminal {
            terminal.close().await;
        }
        if let Some(uploads) = &self.uploads {
            uploads.close().await;
        }
        self.sessions.close().await;
    }

    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
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

    pub fn router(self: &Arc<Self>, secure: bool) -> Router {
        let guard_state = GuardState {
            gateway: Arc::clone(self),
            secure,
        };
        Router::new()
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
            .merge(routes::router())
            .fallback(routes::not_found)
            .layer(middleware::from_fn_with_state(guard_state, auth::guard))
            .with_state(Arc::clone(self))
    }
}
