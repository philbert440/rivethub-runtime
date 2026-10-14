pub mod auth;
pub mod error;
pub mod extract;
pub mod routes;
pub mod server;
pub mod sessions;
pub mod tenancy;
pub mod term;
pub mod tls;

use std::sync::Arc;

use axum::Router;
use axum::middleware;
use axum::routing;
use kernel::agent::TurnRunner;
use kernel::memory::MemoryStore;

use crate::auth::DenialLog;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub hostname: String,
    pub node: String,
    pub allowed_origins: Vec<String>,
    pub allowed_hosts: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct GuardState {
    pub(crate) gateway: Arc<Gateway>,
    pub(crate) secure: bool,
}

impl Settings {
    #[must_use]
    pub fn from_config(config: &config::Config, hostname: String, node: String) -> Self {
        let section = config.gateway.clone().unwrap_or_default();
        Self {
            hostname,
            node,
            allowed_origins: section.allowed_origins,
            allowed_hosts: section.allowed_hosts,
        }
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
}

impl Gateway {
    #[must_use]
    pub fn new(store: Arc<dyn MemoryStore>, settings: Settings) -> Self {
        let policy = auth::OriginPolicy::new(
            settings.allowed_origins.clone(),
            settings.allowed_hosts.clone(),
        );
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
        }
    }

    #[must_use]
    pub fn with_turn_runner(mut self, runner: Arc<dyn TurnRunner>) -> Self {
        self.turn_runner = Some(runner);
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
