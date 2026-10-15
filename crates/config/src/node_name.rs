use crate::ConfigError;
use crate::Environment;
use crate::model::Config;

pub fn resolve(config: &Config, env: &Environment, hostname: &str) -> Result<String, ConfigError> {
    if let Some(name) = mesh_name(config) {
        return Ok(name);
    }
    env.node_name(hostname)
}

fn mesh_name(config: &Config) -> Option<String> {
    let raw = config
        .mesh
        .as_ref()
        .and_then(|mesh| mesh.node_name.as_deref())?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}
