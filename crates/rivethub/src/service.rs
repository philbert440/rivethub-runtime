use std::io;
use std::sync::Arc;

use config::Config;
use config::Environment;
use config::Finding;
use gateway::Gateway;
use gateway::Settings;
use gateway::server::BindError;
use gateway::server::BindSource;
use gateway::server::TlsPaths;
use kernel::memory::MemoryStore;
use memory_postgres::PostgresMemory;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::signal::unix::SignalKind;
use tokio_util::sync::CancellationToken;

use crate::Exit;

#[derive(Debug, Error)]
pub enum StartupError {
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error(
        "no Postgres URL for the memory database — set \"memory.postgres.connection_string\" or \"RIVETOS_PG_URL\""
    )]
    Url,
    #[error(transparent)]
    Memory(#[from] memory_postgres::Error),
    #[error(transparent)]
    Bind(#[from] BindError),
    #[error("the gateway listener failed: {0}")]
    Io(#[from] io::Error),
}

pub async fn connect(
    config: &Config,
    env: &Environment,
) -> Result<Arc<dyn MemoryStore>, StartupError> {
    let url = config.memory_postgres_url(env).ok_or(StartupError::Url)?;
    let memory = PostgresMemory::connect(&url).await?;
    Ok(Arc::new(memory))
}

pub fn compose(
    config: &Config,
    env: &Environment,
    store: Arc<dyn MemoryStore>,
) -> Result<Arc<Gateway>, StartupError> {
    let hostname = host_name();
    let settings = Settings::from_config(config, hostname.clone(), env.node_name(&hostname));
    Ok(Arc::new(Gateway::new(store, settings)))
}

pub async fn listen(config: &Config) -> Result<(TcpListener, Option<TlsPaths>), StartupError> {
    let source = BindSource::from_config(config);
    let tls = gateway::server::resolve_tls(&source)?;
    let listener = gateway::server::bind(&source).await?;
    Ok((listener, tls))
}

pub async fn serve(
    listener: TcpListener,
    tls: Option<TlsPaths>,
    gateway: Arc<Gateway>,
    shutdown: CancellationToken,
) -> Result<(), StartupError> {
    Ok(gateway::server::serve(listener, gateway, tls, shutdown).await?)
}

pub fn load(env: &Environment) -> Result<Config, StartupError> {
    if let Some(location) = config::resolve_config_path(env) {
        let loaded = Config::load(env, &location)?;
        for finding in &loaded.warnings {
            tracing::warn!("{}", finding_line(finding));
        }
        for notice in &loaded.notices {
            tracing::warn!("{notice}");
        }
        return Ok(loaded.config);
    }
    tracing::warn!("no config file found — running with the built-in defaults");
    Ok(Config::default())
}

#[must_use]
pub fn host_name() -> String {
    match nix::unistd::gethostname() {
        Ok(name) => {
            let name = name.to_string_lossy().trim().to_owned();
            if name.is_empty() {
                "local".to_owned()
            } else {
                name
            }
        }
        Err(error) => {
            tracing::warn!("gethostname failed ({error}) — using \"local\"");
            "local".to_owned()
        }
    }
}

async fn run(env: &Environment, shutdown: CancellationToken) -> Result<(), StartupError> {
    let config = load(env)?;
    let store = connect(&config, env).await?;
    let gateway = compose(&config, env, store)?;
    let (listener, tls) = listen(&config).await?;
    let addr = listener.local_addr()?;
    tracing::info!(
        "gateway listening on {addr}{}",
        if tls.is_some() { " (tls)" } else { " (plain)" }
    );
    serve(listener, tls, gateway, shutdown).await
}

pub async fn start(env: &Environment) -> Exit {
    let shutdown = CancellationToken::new();
    let mut sigterm = match tokio::signal::unix::signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            tracing::error!("installing the SIGTERM handler failed: {error}");
            return Exit::Runtime;
        }
    };
    let token = shutdown.clone();
    tokio::spawn(async move {
        let signal = tokio::select! {
            _ = tokio::signal::ctrl_c() => "SIGINT",
            _ = sigterm.recv() => "SIGTERM",
        };
        tracing::info!("{signal} received — shutting down");
        token.cancel();
    });
    match run(env, shutdown).await {
        Ok(()) => Exit::Ok,
        Err(error) => {
            report(&error);
            Exit::from_startup(&error)
        }
    }
}

fn report(error: &StartupError) {
    if let StartupError::Config(config::ConfigError::Validation { result }) = error {
        for finding in &result.errors {
            tracing::error!("{}", finding_line(finding));
        }
        return;
    }
    tracing::error!("{error}");
}

fn finding_line(finding: &Finding) -> String {
    if finding.path.is_empty() {
        finding.message.clone()
    } else {
        format!("[{}] {}", finding.path, finding.message)
    }
}
