use std::path::{Path, PathBuf};

use rmcp::model::JsonObject;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("{path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("{path}: the script records no calls")]
    NoCalls { path: PathBuf },
    #[error("{path}: call {index}: the tool name is empty")]
    EmptyName { path: PathBuf, index: usize },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    pub calls: Vec<ScriptCall>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptCall {
    pub name: String,
    #[serde(default)]
    pub arguments: Option<JsonObject>,
    #[serde(default)]
    pub note: Option<String>,
}

pub fn load(path: &Path) -> Result<Script, ScriptError> {
    let owned = path.to_path_buf();
    let text = std::fs::read_to_string(path).map_err(|source| ScriptError::Read {
        path: owned,
        source,
    })?;
    let script: Script = serde_json::from_str(&text).map_err(|source| ScriptError::Parse {
        path: path.to_path_buf(),
        source,
    })?;
    validate(path, &script)?;
    Ok(script)
}

fn validate(path: &Path, script: &Script) -> Result<(), ScriptError> {
    if script.calls.is_empty() {
        return Err(ScriptError::NoCalls {
            path: path.to_path_buf(),
        });
    }
    for (index, call) in script.calls.iter().enumerate() {
        if call.name.is_empty() {
            return Err(ScriptError::EmptyName {
                path: path.to_path_buf(),
                index,
            });
        }
    }
    Ok(())
}
