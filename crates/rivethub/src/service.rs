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
    #[error(transparent)]
    Node(#[from] crate::NodeError),
    #[error(transparent)]
    Settings(#[from] gateway::SettingsError),
    #[error(
        "no Postgres URL for the memory database — set \"memory.postgres.connection_string\" or \"{POSTGRES_URL_VARIABLE}\""
    )]
    Url,
    #[error(transparent)]
    Memory(#[from] memory_postgres::Error),
    #[error(transparent)]
    Close(#[from] kernel::memory::MemoryStoreError),
    #[error("memory.postgres.embed_model is required when embed_endpoint is set")]
    EmbedModel,
    #[error(transparent)]
    EmbedProvider(#[from] providers::BuildError),
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
    #[error(transparent)]
    Voice(#[from] gateway::VoiceConfigError),
}

pub(crate) fn postgres_url(config: &Config, env: &Environment) -> Result<String, StartupError> {
    if config.sqlite_selected() {
        return Err(StartupError::SqliteUnavailable);
    }
    config.memory_postgres_url(env).ok_or(StartupError::Url)
}

pub(crate) async fn postgres(
    config: &Config,
    env: &Environment,
) -> Result<PostgresMemory, StartupError> {
    let url = postgres_url(config, env)?;
    let embed = query_embed(config)?;
    let memory = PostgresMemory::connect(&url).await?;
    Ok(match embed {
        Some(embed) => memory.with_query_embed(embed),
        None => memory,
    })
}

fn query_embed(config: &Config) -> Result<Option<memory_postgres::QueryEmbed>, StartupError> {
    let Some(section) = config
        .memory
        .as_ref()
        .and_then(|memory| memory.postgres.as_ref())
    else {
        return Ok(None);
    };
    let Some(endpoint) = section.embed_endpoint.clone() else {
        return Ok(None);
    };
    let model = section
        .embed_model
        .clone()
        .ok_or(StartupError::EmbedModel)?;
    let provider = providers::OpenAiCompatible::build(
        config::ProviderType::OpenaiCompatible,
        &config::provider::OpenaiProvider {
            base_url: Some(endpoint),
            model: Some(model),
            api_key: section.embed_api_key.clone(),
            probe_models: Some(false),
            ..config::provider::OpenaiProvider::default()
        },
    )?;
    Ok(Some(memory_postgres::QueryEmbed::new(
        Arc::new(provider),
        section.embed_query_instruction.clone(),
        section.embed_timeout_ms,
        section.hnsw_ef_search,
    )))
}

pub async fn connect(
    config: &Config,
    env: &Environment,
) -> Result<Arc<dyn MemoryStore>, StartupError> {
    Ok(Arc::new(postgres(config, env).await?))
}

pub fn compose(
    config: &Config,
    env: &Environment,
    store: Arc<dyn MemoryStore>,
) -> Result<Arc<Gateway>, StartupError> {
    let node = crate::identity::announce_resolved("gateway", config, env)?;
    let settings = Settings::from_config_in(config, env, node.hostname, node.name)?;
    let gateway = Gateway::new(store, settings).with_uploads(uploads(env)?);
    let gateway = with_voice_and_audio(gateway, config, env)?;
    Ok(Arc::new(gateway))
}

pub(crate) fn with_voice_and_audio(
    mut gateway: Gateway,
    config: &Config,
    env: &Environment,
) -> Result<Gateway, crate::composition::ComposeError> {
    let audio = env.audio_switch();
    if let Some(ignored) = &audio.ignored {
        tracing::warn!("{ignored}");
    }
    if config.terminal_enabled() || audio.enabled {
        let directory = config::Paths::new(env)?.state_dir().join("audio");
        gateway = gateway.with_audio(gateway::AudioConfig::new(directory));
    }
    let voice = env.voice_endpoints();
    Ok(gateway.with_voice(gateway::VoiceSettings::new(
        voice.stt_url,
        voice.tts_url,
        voice.instructions,
    ))?)
}

pub(crate) fn uploads(
    env: &Environment,
) -> Result<gateway::uploads::UploadService, config::ConfigError> {
    let uploads = env.upload_settings();
    for ignored in &uploads.ignored {
        tracing::warn!("ignored upload limit {ignored}");
    }
    Ok(gateway::uploads::UploadService::new(
        gateway::uploads::UploadOptions {
            root: config::Paths::new(env)?.state_dir().join("uploads"),
            max_bytes: uploads.max_bytes,
            ttl: uploads.ttl,
        },
    ))
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
    if let Some(location) = config::resolve_config_path(env)? {
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

async fn run(env: &Environment, shutdown: CancellationToken) -> Result<(), StartupError> {
    let _cancel_on_drop = shutdown.clone().drop_guard();
    if let Some(notice) = move_legacy_state(env).await? {
        tracing::warn!("{notice}");
    }
    let owned_env = env.clone();
    let config = tokio::task::spawn_blocking(move || load(&owned_env))
        .await
        .map_err(StartupError::StateTask)??;
    let memory = tokio::select! {
        () = shutdown.cancelled() => return Ok(()),
        memory = postgres(&config, env) => Arc::new(memory?),
    };
    let result = run_gateway(&config, env, memory.clone(), &shutdown).await;
    shutdown.cancel();
    let closed = memory.close().await;
    closed?;
    result?;
    tracing::info!("gateway stopped");
    Ok(())
}

async fn run_gateway(
    config: &Config,
    env: &Environment,
    store: Arc<dyn MemoryStore>,
    shutdown: &CancellationToken,
) -> Result<(), StartupError> {
    let mut composition =
        crate::composition::Composition::build(config, env, store, shutdown).await?;
    let listening = async {
        let (listener, tls) = listen(config).await?;
        let addr = listener.local_addr()?;
        tracing::info!(
            "gateway listening on {addr}{}",
            if tls.is_some() { " (tls)" } else { " (plain)" }
        );
        Ok::<_, StartupError>((listener, tls))
    }
    .await;
    let (listener, tls) = match listening {
        Ok(bound) => bound,
        Err(error) => {
            composition.close().await?;
            return Err(error);
        }
    };
    let serving = gateway::server::serve_router_with(
        listener,
        composition.router(tls.is_some()),
        tls,
        shutdown.clone(),
        gateway::server::ServeOptions::default(),
    );
    tokio::pin!(serving);
    let (served, failed) = tokio::select! {
        biased;
        () = shutdown.cancelled() => (None, None),
        result = &mut serving => (Some(result), None),
        result = composition.failed() => (None, Some(result)),
    };
    shutdown.cancel();
    let closed = composition.close().await;
    let served = match served {
        Some(result) => result,
        None => serving.await,
    };
    closed?;
    if let Some(result) = failed {
        result?;
    }
    served?;
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
    let signals = match crate::signals::install() {
        Ok(signals) => signals,
        Err(exit) => return exit,
    };
    let result = run(env, signals.token()).await;
    if let Err(exit) = signals.finish().await {
        return exit;
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
    crate::identity::announce_resolved("gateway", &config, env)?;
    Ok(())
}

async fn move_legacy_state(env: &Environment) -> Result<Option<String>, StartupError> {
    let env = env.clone();
    let moved = tokio::task::spawn_blocking(move || config::adopt_legacy_state(&env))
        .await
        .map_err(StartupError::StateTask)?;
    Ok(moved?)
}

pub(crate) fn report(error: &StartupError) {
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

#[cfg(test)]
mod embed_tests {
    use super::*;

    fn config(memory: &str) -> Result<Config, Box<dyn std::error::Error>> {
        let text = format!(
            "runtime:\n  workspace: /tmp\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:9/v1\n    model: m\nmemory:\n  backend: postgres\n  postgres:\n    connection_string: postgres://u@127.0.0.1:9/db\n{memory}"
        );
        Ok(Config::from_str(&Environment::default(), &text)?.config)
    }

    #[test]
    fn without_an_endpoint_search_stays_keyword_only() -> Result<(), Box<dyn std::error::Error>> {
        assert!(query_embed(&config("")?)?.is_none());
        Ok(())
    }

    #[test]
    fn an_endpoint_without_a_model_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let refused = query_embed(&config("    embed_endpoint: http://127.0.0.1:9\n")?);
        assert!(matches!(refused, Err(StartupError::EmbedModel)));
        Ok(())
    }

    #[test]
    fn an_endpoint_and_model_give_search_a_query_embedder() -> Result<(), Box<dyn std::error::Error>>
    {
        let embed = query_embed(&config(
            "    embed_endpoint: http://127.0.0.1:9\n    embed_model: text-embedding-3-small\n",
        )?)?;
        assert!(embed.is_some());
        Ok(())
    }
}
