mod ensure;

use std::io::Write;
use std::path::{Path, PathBuf};

use config::model::GatewaySection;
use config::{Config, ConfigLocation, Environment};

use crate::Error;

pub(crate) use ensure::{Ensure, ensure};

const DEFAULT_GATEWAY_PORT: u16 = 5174;

#[derive(Debug)]
pub struct Prepared {
    pub config: Config,
    pub claim: config::NodeClaim,
    pub database: PathBuf,
}

impl Prepared {
    pub fn release(self) -> Result<(), Error> {
        Ok(self.claim.release()?)
    }
}

pub(crate) async fn prepare(
    env: &Environment,
    port: Option<u16>,
    no_lan: bool,
) -> Result<Prepared, Error> {
    if port == Some(0) {
        return Err(Error::GatewayPort);
    }
    let owned = env.clone();
    tokio::task::spawn_blocking(move || prepare_blocking(&owned, port, no_lan)).await?
}

fn prepare_blocking(env: &Environment, port: Option<u16>, no_lan: bool) -> Result<Prepared, Error> {
    if let Some(notice) = config::adopt_legacy_state(env)? {
        tracing::warn!("{notice}");
    }
    let found = config::resolve_config_path(env)?;
    let state = config::Paths::new(env)?.state_dir();
    let claim = config::claim_node(&state, std::process::id())?;
    let guard = ClaimGuard { claim: Some(claim) };
    let location = match found {
        Some(location) => location,
        None => write_generated(&state, port, no_lan)?,
    };
    let mut loaded = config::Config::load(env, &location, config::Profile::Standalone)?;
    for finding in &loaded.warnings {
        tracing::warn!("[{}] {}", finding.path, finding.message);
    }
    for notice in &loaded.notices {
        tracing::warn!("{notice}");
    }
    apply_bind(&mut loaded.config, port, no_lan);
    let database = sqlite_file(&loaded.config, env)?;
    migrate::sqlite(&database).map_err(Error::Migrate)?;
    let claim = guard.into_claim()?;
    Ok(Prepared {
        config: loaded.config,
        claim,
        database,
    })
}

fn write_generated(state: &Path, port: Option<u16>, no_lan: bool) -> Result<ConfigLocation, Error> {
    let database = state.join("memory.sqlite");
    let yaml = config::standalone_config_yaml(&database, bind_host(no_lan), bind_port(port))?;
    let destination = state.join("config.yaml");
    refuse_existing(&destination)?;
    let partial = state.join(".config.yaml.partial");
    publish(&partial, &destination, yaml.as_bytes())?;
    Ok(ConfigLocation::Current(destination))
}

fn refuse_existing(destination: &Path) -> Result<(), Error> {
    match std::fs::symlink_metadata(destination) {
        Ok(_) => Err(Error::ConfigPresent {
            path: destination.to_path_buf(),
        }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(source.into()),
    }
}

fn publish(partial: &Path, destination: &Path, bytes: &[u8]) -> Result<(), Error> {
    match std::fs::remove_file(partial) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => return Err(source.into()),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(partial)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(partial, destination)?;
    Ok(())
}

fn apply_bind(config: &mut Config, port: Option<u16>, no_lan: bool) {
    if port.is_none() && !no_lan {
        return;
    }
    let section = config.gateway.get_or_insert_with(GatewaySection::default);
    if let Some(port) = port {
        section.port = port;
    } else if section.port == 0 {
        section.port = DEFAULT_GATEWAY_PORT;
    }
    if no_lan {
        section.host = Some("127.0.0.1".to_owned());
    }
}

fn sqlite_file(config: &Config, env: &Environment) -> Result<PathBuf, Error> {
    if config.postgres_selected(env) {
        return Err(Error::PostgresBackend);
    }
    let Some(path) = config
        .memory
        .as_ref()
        .and_then(|memory| memory.sqlite.as_ref())
        .and_then(|sqlite| sqlite.path.clone())
    else {
        return if config.pglite_import_pending() {
            Ok(config::Paths::new(env)?.state_dir().join("memory.sqlite"))
        } else {
            Err(Error::LocalDatabase)
        };
    };
    if path.as_os_str().is_empty() || path == Path::new(":memory:") {
        return Err(Error::LocalDatabase);
    }
    Ok(path)
}

fn bind_host(no_lan: bool) -> &'static str {
    if no_lan { "127.0.0.1" } else { "0.0.0.0" }
}

fn bind_port(port: Option<u16>) -> u16 {
    match port {
        Some(port) => port,
        None => DEFAULT_GATEWAY_PORT,
    }
}

struct ClaimGuard {
    claim: Option<config::NodeClaim>,
}

impl ClaimGuard {
    fn into_claim(mut self) -> Result<config::NodeClaim, Error> {
        self.claim.take().ok_or(Error::ClaimMissing)
    }
}

impl Drop for ClaimGuard {
    fn drop(&mut self) {
        if let Some(claim) = self.claim.take() {
            match claim.release() {
                Ok(()) => {}
                Err(error) => tracing::error!("pid file removal failed: {error}"),
            }
        }
    }
}
