use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::environment::Environment;
use crate::model::Config;
use crate::paths::Paths;
use crate::provider::{
    CliProvider, OpenaiProvider, ProviderBody, ProviderSlot, ProviderType, UnknownProvider,
};
use crate::report::{Finding, ValidationResult, format_validation_result};
use crate::resolve::resolve_env_vars_with;
use crate::validate;

const PGLITE_IMPORT_NOTICE: &str = "memory.postgres.embedded is set — the embedded Postgres (PGlite) option was removed; the memory backend resolves to SQLite and the one-time import runs at first start.";

#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<Finding>,
    pub notices: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
    #[error("\"{path}\" has the wrong type: {message}")]
    Type { path: String, message: String },
    #[error("Config validation failed")]
    Validation { formatted: String },
}

#[derive(Debug)]
pub struct Typed {
    pub config: Config,
    pub ignored: Vec<String>,
}

#[derive(Debug)]
pub struct TypeConflict {
    pub path: String,
    pub message: String,
}

pub fn resolve_config_path(env: &Environment) -> Option<(PathBuf, bool)> {
    let paths = Paths::new(env);
    for (candidate, legacy) in paths.config_candidates() {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Some((path, legacy));
        }
    }
    None
}

pub fn validate_document(document: &serde_yaml::Value) -> ValidationResult {
    let presence = validate::Presence {
        plugins: has_key(document, "plugins"),
        workflows: has_key(document, "workflows"),
    };
    match type_document(document) {
        Ok(typed) => validate::validate(&typed.config, &typed.ignored, &presence),
        Err(conflict) => {
            let mut out = ValidationResult::default();
            let TypeConflict { path, message } = conflict;
            out.error(&path, format!("\"{path}\" has the wrong type: {message}"));
            out
        }
    }
}

fn has_key(document: &serde_yaml::Value, key: &str) -> bool {
    document.get(key).is_some()
}

pub fn type_document(document: &serde_yaml::Value) -> Result<Typed, TypeConflict> {
    let mut ignored = Vec::new();
    let mut config = deserialize_main(document, &mut ignored)?;
    config.providers = provider_map(document, &mut ignored)?;
    Ok(Typed { config, ignored })
}

fn deserialize_main(
    document: &serde_yaml::Value,
    ignored: &mut Vec<String>,
) -> Result<Config, TypeConflict> {
    if matches!(document, serde_yaml::Value::Null) {
        return Err(TypeConflict {
            path: String::new(),
            message: "invalid type: null, expected struct Config".to_owned(),
        });
    }
    tracked::<Config>(document, "", ignored)
}

fn tracked<T: DeserializeOwned>(
    raw: &serde_yaml::Value,
    prefix: &str,
    ignored: &mut Vec<String>,
) -> Result<T, TypeConflict> {
    let mut collect = |path: serde_ignored::Path<'_>| ignored.push(ignored_path(prefix, &path));
    serde_path_to_error::deserialize(serde_ignored::Deserializer::new(raw.clone(), &mut collect))
        .map_err(|error| conflict_at(&error, prefix))
}

fn ignored_path(prefix: &str, path: &serde_ignored::Path<'_>) -> String {
    let relative = path.to_string();
    if prefix.is_empty() {
        relative
    } else {
        format!("{prefix}.{relative}")
    }
}

fn provider_map(
    document: &serde_yaml::Value,
    ignored: &mut Vec<String>,
) -> Result<Option<BTreeMap<String, ProviderSlot>>, TypeConflict> {
    let Some(entries) = document.get("providers") else {
        return Ok(None);
    };
    if entries.is_null() {
        return Ok(None);
    }
    let entries: BTreeMap<String, serde_yaml::Value> =
        serde_path_to_error::deserialize(entries.clone())
            .map_err(|error| conflict_at(&error, "providers"))?;
    let mut map = BTreeMap::new();
    for (id, raw) in entries {
        let prefix = format!("providers.{id}");
        let declared = declared_type(&raw, &prefix)?;
        let kind = match declared.as_deref() {
            Some(text) => ProviderType::from_str(text).ok(),
            None => ProviderType::from_str(&id).ok(),
        };
        let body = match kind {
            Some(kind) if kind.is_removed() || kind == ProviderType::OpenaiCompat => {
                ProviderBody::Removed
            }
            Some(kind) if kind.is_openai_family() => {
                ProviderBody::Openai(tracked::<OpenaiProvider>(&raw, &prefix, ignored)?)
            }
            Some(kind) if kind.is_cli_family() => {
                ProviderBody::Cli(tracked::<CliProvider>(&raw, &prefix, ignored)?)
            }
            Some(_) => ProviderBody::Removed,
            None => ProviderBody::Unknown(tracked::<UnknownProvider>(&raw, &prefix, ignored)?),
        };
        map.insert(id, ProviderSlot { kind, body });
    }
    Ok(Some(map))
}

#[derive(Debug, Default, Deserialize)]
struct DeclaredType {
    #[serde(default, rename = "type")]
    declared: Option<String>,
}

fn declared_type(raw: &serde_yaml::Value, prefix: &str) -> Result<Option<String>, TypeConflict> {
    serde_path_to_error::deserialize::<serde_yaml::Value, DeclaredType>(raw.clone())
        .map(|tag| tag.declared)
        .map_err(|error| conflict_at(&error, prefix))
}

fn conflict_at<E: std::fmt::Display>(
    error: &serde_path_to_error::Error<E>,
    prefix: &str,
) -> TypeConflict {
    let relative = error.path().to_string();
    let path = if relative.is_empty() || relative == "." {
        prefix.to_owned()
    } else if prefix.is_empty() {
        relative
    } else {
        format!("{prefix}.{relative}")
    };
    TypeConflict {
        path,
        message: error.inner().to_string(),
    }
}

impl Config {
    pub fn load(env: &Environment, path: &Path) -> Result<Loaded, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut loaded = Self::from_str(env, &text)?;
        if is_legacy_path(path) {
            loaded.notices.insert(
                0,
                format!(
                    "Using legacy config path {} — move it to {} so both runtimes agree.",
                    path.display(),
                    Paths::new(env).hub_home()
                ),
            );
        }
        Ok(loaded)
    }

    pub fn from_str(env: &Environment, text: &str) -> Result<Loaded, ConfigError> {
        let document: serde_yaml::Value = serde_yaml::from_str(text)?;
        let result = validate_document(&document);
        if !result.valid() {
            return Err(ConfigError::Validation {
                formatted: format_validation_result(&result),
            });
        }
        let resolved = resolve_env_vars_with(env, &document);
        let typed = type_document(&resolved).map_err(|conflict| ConfigError::Type {
            path: conflict.path,
            message: conflict.message,
        })?;
        let notices = notices(env, &document, &typed.config);
        Ok(Loaded {
            config: typed.config,
            warnings: result.warnings,
            notices,
        })
    }
}

fn is_legacy_path(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy() == ".rivetos")
}

fn notices(env: &Environment, document: &serde_yaml::Value, config: &Config) -> Vec<String> {
    let mut out = env.legacy_notices();
    out.extend(env.deprecation_warnings());
    out.extend(plugin_entry_notices(document));
    if config.pglite_import_pending() {
        out.push(PGLITE_IMPORT_NOTICE.to_owned());
    }
    out
}

fn plugin_entry_notices(document: &serde_yaml::Value) -> Vec<String> {
    document
        .get("plugins")
        .and_then(serde_yaml::Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(serde_yaml::Value::as_str)
        .filter(|entry| entry.starts_with("plugins/tools/"))
        .map(|entry| {
            format!(
                "{entry} is deprecated — built-in tools were removed; every harness ships its own."
            )
        })
        .collect()
}
