use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use wire::plugin::PluginId;
use wire::plugin::manifest::{Permission, PluginManifest};
use wire::plugin::view::ViewValue;

use super::HostError;

pub(super) const MANIFEST_FILE: &str = "rivethub-plugin.json";
const REGISTRY_FILE: &str = "registry.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Stored {
    pub(super) path: PathBuf,
    pub(super) manifest: PluginManifest,
    pub(super) enabled: bool,
    pub(super) granted: Vec<Permission>,
    pub(super) installed_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) settings: BTreeMap<String, ViewValue>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    plugins: BTreeMap<PluginId, Stored>,
}

pub(super) async fn load(root: &Path) -> Result<BTreeMap<PluginId, Stored>, HostError> {
    match tokio::fs::read(root.join(REGISTRY_FILE)).await {
        Ok(bytes) => Ok(serde_json::from_slice::<RegistryFile>(&bytes)?.plugins),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}

pub(super) async fn save(
    root: &Path,
    plugins: BTreeMap<PluginId, Stored>,
) -> Result<(), HostError> {
    let bytes = serde_json::to_vec_pretty(&RegistryFile { plugins })?;
    let staged = root.join(format!("{REGISTRY_FILE}.tmp"));
    tokio::fs::create_dir_all(root).await?;
    tokio::fs::write(&staged, bytes).await?;
    set_private(&staged).await?;
    tokio::fs::rename(&staged, root.join(REGISTRY_FILE)).await?;
    Ok(())
}

pub(super) async fn set_private(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt as _;
    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await
}

pub(super) async fn read_manifest(dir: &Path) -> Result<PluginManifest, HostError> {
    let file = dir.join(MANIFEST_FILE);
    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|error| HostError::Invalid(format!("{}: {error}", file.display())))?;
    let mut decoder = serde_json::Deserializer::from_slice(&bytes);
    let manifest: PluginManifest = serde_path_to_error::deserialize(&mut decoder)
        .map_err(|error| HostError::Invalid(format!("{}: {error}", error.path())))?;
    kernel::plugins::check_manifest(&manifest)
        .map_err(|problem| HostError::Invalid(problem.to_string()))?;
    super::jobs::check_schedules(&manifest)?;
    Ok(manifest)
}
