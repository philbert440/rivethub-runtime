use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use wire::gateway::roster::RosterEntry;
use wire::gateway::roster::RosterFile;
use wire::harness::HarnessId;

use crate::Error;
use crate::session::RosterKey;

pub const ROSTER_FILE_NAME: &str = "den-term.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedRoster {
    pub default_id: String,
    pub commands: Vec<ResolvedCommand>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCommand {
    pub id: String,
    pub label: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub harness: Option<HarnessId>,
    pub session: bool,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RosterError {
    #[error("terminal roster {path} is not valid JSON")]
    Json {
        path: Box<Path>,
        #[source]
        source: serde_json::Error,
    },
    #[error("terminal roster default {default_id} is not a command")]
    UnknownDefault { default_id: String },
    #[error("terminal roster key {0} is not a roster key")]
    InvalidKey(String),
    #[error("terminal roster command {id} has an empty argv")]
    EmptyArgv { id: String },
    #[error("terminal roster {} is not absolute", .0.display())]
    RelativeCwd(Box<Path>),
    #[error("terminal roster {} is not a file", .0.display())]
    NotAFile(Box<Path>),
}

#[must_use]
pub fn resolve_roster_path(
    commands_file: Option<&Path>,
    env: &config::Environment,
    state_dir: &Path,
) -> PathBuf {
    if let Some(path) = commands_file {
        return path.to_path_buf();
    }
    if let Some(path) = env.term_roster_path() {
        return path;
    }
    state_dir.join(ROSTER_FILE_NAME)
}

pub async fn load_roster(path: &Path, fallback_cwd: &Path) -> Result<LoadedRoster, Error> {
    match tokio::fs::symlink_metadata(path).await {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => builtin(fallback_cwd),
        Err(error) => Err(Error::Io(error)),
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
            read_roster(path, fallback_cwd).await
        }
        Ok(_) => Err(RosterError::NotAFile(Box::from(path)).into()),
    }
}

async fn read_roster(path: &Path, fallback_cwd: &Path) -> Result<LoadedRoster, Error> {
    let bytes = tokio::fs::read(path).await?;
    let file =
        serde_json::from_slice::<RosterFile>(&bytes).map_err(|source| RosterError::Json {
            path: Box::from(path),
            source,
        })?;
    resolve_file(file, fallback_cwd)
}

fn resolve_file(file: RosterFile, fallback_cwd: &Path) -> Result<LoadedRoster, Error> {
    let top = match &file.cwd {
        Some(cwd) => Some(absolute_cwd(cwd)?),
        None => None,
    };
    let fallback = absolute_cwd(fallback_cwd)?;
    let mut commands = Vec::with_capacity(file.commands.len());
    for (id, entry) in file.commands {
        commands.push(resolve_command(
            id,
            entry,
            top.as_deref(),
            &fallback,
            &file.env,
        )?);
    }
    if !commands.iter().any(|command| command.id == file.default) {
        return Err(RosterError::UnknownDefault {
            default_id: file.default,
        }
        .into());
    }
    Ok(LoadedRoster {
        default_id: file.default,
        commands,
    })
}

fn resolve_command(
    id: String,
    entry: RosterEntry,
    top: Option<&Path>,
    fallback: &Path,
    top_env: &BTreeMap<String, String>,
) -> Result<ResolvedCommand, Error> {
    if let Err(error) = RosterKey::new(id.clone()) {
        let Error::InvalidRosterKey(value) = error else {
            return Err(error);
        };
        return Err(RosterError::InvalidKey(value).into());
    }
    if entry.argv.is_empty() {
        return Err(RosterError::EmptyArgv { id }.into());
    }
    let cwd = match (entry.session == Some(true), &entry.cwd) {
        (true, Some(cwd)) if id == "opencode" => absolute_cwd(cwd)?,
        (true, _) => fallback.to_path_buf(),
        (false, Some(cwd)) => absolute_cwd(cwd)?,
        (false, None) => match top {
            Some(cwd) => cwd.to_path_buf(),
            None => fallback.to_path_buf(),
        },
    };
    Ok(ResolvedCommand {
        harness: wire::gateway::roster::harness_for_roster_key(&id),
        session: entry.session == Some(true),
        env: overlay(top_env, &entry.env),
        id,
        label: entry.label,
        argv: entry.argv,
        cwd,
    })
}

fn overlay(
    top: &BTreeMap<String, String>,
    entry: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut env = top.clone();
    for (name, value) in entry {
        env.insert(name.clone(), value.clone());
    }
    env
}

fn absolute_cwd(path: &Path) -> Result<PathBuf, Error> {
    if path.is_relative() {
        return Err(RosterError::RelativeCwd(Box::from(path)).into());
    }
    Ok(path.to_path_buf())
}

fn builtin(fallback_cwd: &Path) -> Result<LoadedRoster, Error> {
    let cwd = absolute_cwd(fallback_cwd)?;
    let commands = wire::gateway::roster::builtin_commands()
        .iter()
        .map(|command| ResolvedCommand {
            id: command.id.to_owned(),
            label: command.label.to_owned(),
            argv: command.argv.iter().map(|arg| (*arg).to_owned()).collect(),
            cwd: cwd.clone(),
            harness: wire::gateway::roster::harness_for_roster_key(command.id),
            session: command.session,
            env: BTreeMap::new(),
        })
        .collect();
    Ok(LoadedRoster {
        default_id: wire::gateway::roster::BUILTIN_DEFAULT_ID.to_owned(),
        commands,
    })
}
