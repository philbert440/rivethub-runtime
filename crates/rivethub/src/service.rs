use std::io;
use std::sync::Arc;

use config::Config;
use config::Environment;
use config::Finding;
use config::POSTGRES_URL_VARIABLE;
use gateway::Gateway;
use gateway::Settings;
use gateway::server::BindError;
use gateway::server::BindSource;
use gateway::server::ServeError;
use gateway::tls::TlsPaths;
use kernel::memory::MemoryStore;
use memory_postgres::PostgresMemory;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::signal::unix::SignalKind;
use tokio_util::sync::CancellationToken;

use crate::Exit;

#[derive(Debug, Error)]
pub enum StartupError {
    #[error("SQLite gateway composition requires a complete SQLite MemoryStore adapter")]
    SqliteUnavailable,
    #[error(transparent)]
    Composition(#[from] crate::composition::ComposeError),
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error(
        "no Postgres URL for the memory database — set \"memory.postgres.connection_string\" or \"{POSTGRES_URL_VARIABLE}\""
    )]
    Url,
    #[error(transparent)]
    Memory(#[from] memory_postgres::Error),
    #[error(transparent)]
    Bind(#[from] BindError),
    #[error(transparent)]
    Serve(#[from] ServeError),
    #[error("the gateway listener failed: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    State(#[from] config::StateError),
    #[error("the legacy state move task aborted")]
    StateTask(#[source] tokio::task::JoinError),
}

pub async fn connect(
    config: &Config,
    env: &Environment,
) -> Result<Arc<dyn MemoryStore>, StartupError> {
    if config.memory.as_ref().is_some_and(|memory| {
        memory.backend == Some(config::MemoryBackend::Sqlite)
            || (memory.backend.is_none() && memory.sqlite.is_some() && memory.postgres.is_none())
    }) || config.pglite_import_pending()
    {
        return Err(StartupError::SqliteUnavailable);
    }
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
    let settings =
        Settings::from_config_in(config, env, hostname.clone(), env.node_name(&hostname));
    let gateway = Gateway::new(store, settings).with_uploads(uploads(env));
    Ok(Arc::new(gateway))
}

pub(crate) fn uploads(env: &Environment) -> gateway::uploads::UploadService {
    let uploads = env.upload_settings();
    for ignored in &uploads.ignored {
        tracing::warn!("ignored upload limit {ignored}");
    }
    gateway::uploads::UploadService::new(gateway::uploads::UploadOptions {
        root: config::Paths::new(env).state_dir().join("uploads"),
        max_bytes: uploads.max_bytes,
        ttl: uploads.ttl,
    })
}

pub async fn listen(config: &Config) -> Result<(TcpListener, Option<TlsPaths>), StartupError> {
    let source = BindSource::from_config(config);
    let tls_source = source.clone();
    let tls = tokio::task::spawn_blocking(move || {
        let tls = gateway::tls::resolve_tls(&tls_source)?;
        if let Some(paths) = &tls {
            gateway::tls::validate(paths).map_err(ServeError::Tls)?;
        }
        Ok::<_, StartupError>(tls)
    })
    .await
    .map_err(StartupError::StateTask)??;
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
    warn_environment(env);
    tracing::warn!("no config file found — running with the built-in defaults");
    Ok(Config::default())
}

fn warn_environment(env: &Environment) {
    for notice in env.legacy_notices() {
        tracing::warn!("{notice}");
    }
    for warning in env.deprecation_warnings() {
        tracing::warn!("{warning}");
    }
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
    let _cancel_on_drop = shutdown.clone().drop_guard();
    if let Some(notice) = move_legacy_state(env).await? {
        tracing::warn!("{notice}");
    }
    let owned_env = env.clone();
    let config = tokio::task::spawn_blocking(move || load(&owned_env))
        .await
        .map_err(StartupError::StateTask)??;
    let store = tokio::select! {
        () = shutdown.cancelled() => return Ok(()),
        store = connect(&config, env) => store?,
    };
    let mut composition =
        crate::composition::Composition::build(&config, env, store, &shutdown).await?;
    let (listener, tls) = listen(&config).await?;
    let addr = listener.local_addr()?;
    tracing::info!(
        "gateway listening on {addr}{}",
        if tls.is_some() { " (tls)" } else { " (plain)" }
    );
    let serving = gateway::server::serve_router_with(
        listener,
        composition.router(tls.is_some()),
        tls,
        shutdown.clone(),
        gateway::server::ServeOptions::default(),
    );
    tokio::pin!(serving);
    let result = tokio::select! {
        biased;
        () = shutdown.cancelled() => { composition.close().await?; serving.await },
        result = &mut serving => { shutdown.cancel(); composition.close().await?; result },
        result = composition.failed() => { shutdown.cancel(); composition.close().await?; result?; serving.await },
    };
    result?;
    tracing::info!("gateway stopped");
    Ok(())
}

pub async fn start(env: &Environment) -> Exit {
    start_with_check(env, false).await
}

pub async fn start_with_check(env: &Environment, check: bool) -> Exit {
    if check {
        return match check_config(env).await {
            Ok(()) => {
                tracing::info!("gateway configuration valid");
                Exit::Ok
            }
            Err(error) => {
                report(&error);
                error.exit()
            }
        };
    }
    let shutdown = CancellationToken::new();
    let mut sigterm = match tokio::signal::unix::signal(SignalKind::terminate()) {
        Ok(signal) => signal,
        Err(error) => {
            tracing::error!("installing the SIGTERM handler failed: {error}");
            return Exit::Runtime;
        }
    };
    let token = shutdown.clone();
    let signals = tokio::spawn(async move {
        let signal = tokio::select! {
            result = tokio::signal::ctrl_c() => result.map(|()| "SIGINT"),
            signal = sigterm.recv() => signal.map(|()| "SIGTERM").ok_or_else(|| io::Error::other("SIGTERM handler closed")),
        };
        token.cancel();
        let signal = signal?;
        tracing::info!("{signal} received — shutting down");
        Ok::<(), io::Error>(())
    });
    let result = run(env, shutdown.clone()).await;
    shutdown.cancel();
    signals.abort();
    match signals.await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::error!("signal handler failed: {error}");
            return Exit::Runtime;
        }
        Err(error) if error.is_cancelled() => {}
        Err(error) => {
            tracing::error!("signal task failed: {error}");
            return Exit::Runtime;
        }
    }
    match result {
        Ok(()) => Exit::Ok,
        Err(error) => {
            report(&error);
            error.exit()
        }
    }
}

async fn check_config(env: &Environment) -> Result<(), StartupError> {
    let loading_env = env.clone();
    let config = tokio::task::spawn_blocking(move || {
        let config = load(&loading_env)?;
        let source = BindSource::from_config(&config);
        if let Some(tls) = gateway::tls::resolve_tls(&source)? {
            gateway::tls::validate(&tls).map_err(ServeError::Tls)?;
        }
        Ok::<_, StartupError>(config)
    })
    .await
    .map_err(StartupError::StateTask)??;
    crate::composition::Composition::validate_users(&config, env).await?;
    Ok(())
}

async fn move_legacy_state(env: &Environment) -> Result<Option<String>, StartupError> {
    let env = env.clone();
    let moved = tokio::task::spawn_blocking(move || config::adopt_legacy_state(&env))
        .await
        .map_err(StartupError::StateTask)?;
    Ok(moved?)
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
