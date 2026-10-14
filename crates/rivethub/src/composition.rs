mod identities;
mod mesh;
mod models;
mod terminals;

use std::sync::Arc;
use std::time::Duration;

use config::{Config, Environment};
use gateway::Gateway;
use gateway::sessions::SessionRegistry;
use kernel::memory::MemoryStore;
use thiserror::Error;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Error)]
pub enum ComposeError {
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
    Provider(#[from] providers::BuildError),
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
    cancel: CancellationToken,
}

impl Composition {
    pub async fn validate_users(config: &Config, env: &Environment) -> Result<(), ComposeError> {
        identities::load(config, env).await?;
        Ok(())
    }

    pub async fn failed(&mut self) -> Result<(), ComposeError> {
        tokio::select! {
            result = self.tasks.join_next(), if !self.tasks.is_empty() => {
                if let Some(result) = result { result??; }
                Err(ComposeError::Invalid("mesh task stopped unexpectedly".into()))
            }
            result = self.audit_tasks.join_next(), if !self.audit_tasks.is_empty() => {
                if let Some(result) = result { result??; }
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
    ) -> Result<Self, ComposeError> {
        let hostname = crate::service::host_name();
        let settings = gateway::Settings::from_config_in(
            config,
            env,
            hostname.clone(),
            env.node_name(&hostname),
        )?;
        let sessions = Arc::new(
            SessionRegistry::new(Duration::from_secs(300)).with_shutdown(shutdown.child_token()),
        );
        let tenancy = identities::load(config, env).await?;
        let mut gateway = Gateway::new(store, settings)
            .with_uploads(crate::service::uploads(env))
            .with_tenancy(tenancy.clone())
            .with_sessions(sessions.clone());
        gateway = crate::service::with_voice_and_audio(gateway, config, env)?;
        let home = env
            .home()
            .ok_or_else(|| ComposeError::Invalid("HOME is required".into()))?;
        gateway = gateway.with_transcript_reader(Arc::new(
            capture::transcript::ClaudeTranscriptReader::new(
                std::path::Path::new(home).join(".claude"),
            ),
        ));
        let background = tokio_util::task::TaskTracker::new();
        if config.terminal_enabled() {
            let owner = tenancy.local_user().cloned().ok_or_else(|| {
                ComposeError::Invalid("a local user is required for terminals".into())
            })?;
            gateway = gateway.with_terminal(
                terminals::build(config, env, sessions, &owner, shutdown, &background).await?,
            );
        }
        let audit_cancel = shutdown.child_token();
        let mut audit_tasks = JoinSet::new();
        let configured = models::build(config, env, &audit_cancel, &mut audit_tasks).await?;
        let mut agents = std::collections::BTreeMap::new();
        let v1 = if let Some(configured) = configured {
            agents = configured.agents;
            gateway = gateway.with_turn_runner(configured.runner);
            Some(configured.service)
        } else {
            None
        };
        let tasks = mesh::start(config, env, agents, tenancy, shutdown).await?;
        Ok(Self {
            gateway: Arc::new(gateway),
            v1,
            audit_tasks,
            audit_cancel,
            tasks,
            background,
            cancel: shutdown.clone(),
        })
    }

    #[must_use]
    pub fn gateway(&self) -> &Arc<Gateway> {
        &self.gateway
    }

    pub fn router(&self, secure: bool) -> axum::Router {
        let router = match &self.v1 {
            Some(service) => service.router(&self.gateway, secure),
            None => self.gateway.router(secure),
        };
        router.layer(axum::middleware::from_fn_with_state(
            self.gateway.clone(),
            identities::authorize,
        ))
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
        Ok(())
    }
}
