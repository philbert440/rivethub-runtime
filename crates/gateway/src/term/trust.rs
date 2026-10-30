use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ApiError;

#[derive(Debug, Clone)]
pub struct ClaudeTrust {
    pub config: PathBuf,
    pub roots: Vec<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ClaudeConfig {
    #[serde(default)]
    projects: BTreeMap<String, ClaudeProject>,
    #[serde(flatten)]
    extra: wire::Extra,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeProject {
    #[serde(skip_serializing_if = "Option::is_none")]
    has_trust_dialog_accepted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    has_completed_project_onboarding: Option<bool>,
    #[serde(flatten)]
    extra: wire::Extra,
}

impl ClaudeProject {
    fn trusted(&self) -> bool {
        self.has_trust_dialog_accepted == Some(true)
            && self.has_completed_project_onboarding == Some(true)
    }
}

impl ClaudeTrust {
    pub(super) async fn apply(
        &self,
        allowed: &[PathBuf],
        directory: &Path,
    ) -> Result<(), ApiError> {
        let config = self.config.clone();
        let mut roots = allowed.to_vec();
        roots.extend(self.roots.iter().cloned());
        let directory = directory.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let directory = directory.canonicalize()?;
            if within(&roots, &directory)? {
                seed(&config, &directory)
            } else {
                Ok(())
            }
        })
        .await
        .map_err(ApiError::Task)?
    }
}

fn within(roots: &[PathBuf], directory: &Path) -> Result<bool, ApiError> {
    for root in roots {
        if root.try_exists()? && directory.starts_with(root.canonicalize()?) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn seed(config: &Path, directory: &Path) -> Result<(), ApiError> {
    let key = directory
        .to_str()
        .ok_or_else(|| ApiError::BadRequest("working directory is not UTF-8".into()))?
        .to_owned();
    let mut settings = read(config)?;
    let project = settings.projects.entry(key).or_default();
    if project.trusted() {
        return Ok(());
    }
    project.has_trust_dialog_accepted = Some(true);
    project.has_completed_project_onboarding = Some(true);
    write(config, &settings)
}

fn read(config: &Path) -> Result<ClaudeConfig, ApiError> {
    match std::fs::read(config) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            ApiError::Unavailable(format!(
                "claude config {} is not valid: {error}",
                config.display()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ClaudeConfig::default()),
        Err(error) => Err(error.into()),
    }
}

fn write(config: &Path, settings: &ClaudeConfig) -> Result<(), ApiError> {
    let parent = config
        .parent()
        .ok_or_else(|| ApiError::Unavailable("claude config has no parent directory".into()))?;
    let body = serde_json::to_vec_pretty(settings).map_err(ApiError::Serialize)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(&body)?;
    staged.as_file().sync_all()?;
    staged
        .persist(config)
        .map_err(|error| ApiError::Io(error.error))?;
    Ok(())
}
