use std::path::Path;
use std::path::PathBuf;

use super::check;
use super::error::AgentError;
use super::paths;
use super::registry;
use crate::Gateway;
use crate::term::AgentLaunch;

const ID_MAX: usize = 64;
const ID_TOKEN: &str = "agentId must be a 1-64 token";

pub(crate) async fn resolve(gateway: &Gateway, id: String) -> Result<AgentLaunch, AgentError> {
    if id.len() > ID_MAX || !check::valid_id(&id) {
        return Err(AgentError::BadRequest(ID_TOKEN));
    }
    let preset = gateway.presets().get(id).await?;
    if let Some(node) = check::foreign(&preset, &gateway.settings().node) {
        return Err(check::hosted(&preset.name, node));
    }
    let Some(raw) = preset
        .directory
        .as_deref()
        .map(|directory| directory.trim().trim_end_matches('/'))
        .filter(|directory| !directory.is_empty())
        .map(str::to_owned)
    else {
        return Err(AgentError::Conflict(format!(
            "agent \"{}\" has no directory",
            preset.name
        )));
    };
    let shared = gateway.settings().shared_dir.clone();
    let link = registry::wants_link(preset.shared_link);
    let directory = tokio::task::spawn_blocking(move || prepare(&raw, &shared, link))
        .await
        .map_err(|error| AgentError::Directory(std::io::Error::other(error)))?
        .map_err(directory_failure)?;
    let model = match preset.launch_model() {
        Some(model) => model.to_owned(),
        None => String::new(),
    };
    Ok(AgentLaunch {
        name: preset.name,
        directory,
        harness: preset.harness_id,
        model,
        effort: preset.effort,
    })
}

fn prepare(raw: &str, shared: &Path, link: bool) -> Result<PathBuf, AgentError> {
    let directory = check::directory_str(raw)?;
    paths::prepare_directory(&directory)?;
    paths::ensure_shared_link(&directory, shared, link)?;
    Ok(directory)
}

fn directory_failure(error: AgentError) -> AgentError {
    if matches!(error, AgentError::Directory(_)) {
        error
    } else {
        AgentError::Directory(std::io::Error::other(error.to_string()))
    }
}
