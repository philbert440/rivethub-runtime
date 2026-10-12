use std::collections::BTreeMap;
use std::path::PathBuf;
use std::str::FromStr;

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::environment::Environment;
use crate::model::Config;
use crate::paths::{ConfigLocation, Paths};
use crate::provider::{
    CliProvider, OpenaiProvider, ProviderBody, ProviderSlot, ProviderType, UnknownProvider,
};
use crate::report::{Finding, ValidationResult};
use crate::resolve::resolve_env_vars_with;
use crate::validate::{self, IgnoredPath};

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
    Yaml(#[from] serde_yaml_ng::Error),
    #[error("\"{path}\" has the wrong type: {message}")]
    Type { path: String, message: String },
    #[error("Config validation failed")]
    Validation { result: ValidationResult },
}

#[derive(Debug)]
pub struct Typed {
    pub config: Config,
    pub ignored: Vec<IgnoredPath>,
}

pub fn resolve_config_path(env: &Environment) -> Option<ConfigLocation> {
    Paths::new(env)
        .config_candidates()
        .into_iter()
        .find(|location| location.path().is_file())
}

pub fn validate_document(document: &serde_yaml_ng::Value) -> ValidationResult {
    let presence = validate::Presence {
        plugins: has_key(document, "plugins"),
        workflows: has_key(document, "workflows"),
    };
    match typed_document(document) {
        Ok(typed) => validate::validate(&typed.config, &typed.ignored, &presence),
        Err((path, message)) => {
            let mut out = ValidationResult::default();
            out.error(&path, format!("\"{path}\" has the wrong type: {message}"));
            out
        }
    }
}

pub fn type_document(document: &serde_yaml_ng::Value) -> Result<Typed, ConfigError> {
    typed_document(document).map_err(|(path, message)| ConfigError::Type { path, message })
}

fn typed_document(document: &serde_yaml_ng::Value) -> Result<Typed, (String, String)> {
    let mut ignored = Vec::new();
    let mut config = deserialize_main(document, &mut ignored)?;
    config.providers = provider_map(document, &mut ignored)?;
    Ok(Typed { config, ignored })
}

fn has_key(document: &serde_yaml_ng::Value, key: &str) -> bool {
    document.get(key).is_some()
}

fn deserialize_main(
    document: &serde_yaml_ng::Value,
    ignored: &mut Vec<IgnoredPath>,
) -> Result<Config, (String, String)> {
    if matches!(document, serde_yaml_ng::Value::Null) {
        return Err((
            String::new(),
            "invalid type: null, expected struct Config".to_owned(),
        ));
    }
    tracked::<Config>(document, "", ignored)
}

fn tracked<T: DeserializeOwned>(
    raw: &serde_yaml_ng::Value,
    prefix: &str,
    ignored: &mut Vec<IgnoredPath>,
) -> Result<T, (String, String)> {
    let mut collect =
        |path: serde_ignored::Path<'_>| ignored.push(IgnoredPath::collect(prefix, &path));
    serde_path_to_error::deserialize(serde_ignored::Deserializer::new(raw, &mut collect))
        .map_err(|error| conflict_at(&error, prefix))
}

fn provider_map(
    document: &serde_yaml_ng::Value,
    ignored: &mut Vec<IgnoredPath>,
) -> Result<Option<BTreeMap<String, ProviderSlot>>, (String, String)> {
    let Some(entries) = document.get("providers") else {
        return Ok(None);
    };
    if entries.is_null() {
        return Ok(None);
    }
    let entries: BTreeMap<String, serde_yaml_ng::Value> = serde_path_to_error::deserialize(entries)
        .map_err(|error| conflict_at(&error, "providers"))?;
    let mut map = BTreeMap::new();
    for (id, raw) in &entries {
        let prefix = format!("providers.{id}");
        let declared = declared_type(raw, &prefix)?;
        let kind = match declared.as_deref() {
            Some(text) => ProviderType::from_str(text).ok(),
            None => ProviderType::from_str(id).ok(),
        };
        let body = match kind {
            Some(kind) if kind.is_removed() || kind == ProviderType::OpenaiCompat => {
                ProviderBody::Removed
            }
            Some(kind) if kind.is_openai_family() => {
                ProviderBody::Openai(tracked::<OpenaiProvider>(raw, &prefix, ignored)?)
            }
            Some(kind) if kind.is_cli_family() => {
                ProviderBody::Cli(tracked::<CliProvider>(raw, &prefix, ignored)?)
            }
            Some(_) => ProviderBody::Removed,
            None => ProviderBody::Unknown(tracked::<UnknownProvider>(raw, &prefix, ignored)?),
        };
        map.insert(id.clone(), ProviderSlot { kind, body });
    }
    Ok(Some(map))
}

#[derive(Debug, Default, Deserialize)]
struct DeclaredType {
    #[serde(default, rename = "type")]
    declared: Option<String>,
}

fn declared_type(
    raw: &serde_yaml_ng::Value,
    prefix: &str,
) -> Result<Option<String>, (String, String)> {
    serde_path_to_error::deserialize::<_, DeclaredType>(raw)
        .map(|tag| tag.declared)
        .map_err(|error| conflict_at(&error, prefix))
}

fn conflict_at<E: std::fmt::Display>(
    error: &serde_path_to_error::Error<E>,
    prefix: &str,
) -> (String, String) {
    let relative = error.path().to_string();
    let path = if relative.is_empty() || relative == "." {
        prefix.to_owned()
    } else if prefix.is_empty() {
        relative
    } else {
        format!("{prefix}.{relative}")
    };
    (path, error.inner().to_string())
}

impl Config {
    pub fn load(env: &Environment, location: &ConfigLocation) -> Result<Loaded, ConfigError> {
        let path = location.path();
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut loaded = Self::from_str(env, &text)?;
        if location.is_legacy() {
            loaded.notices.insert(
                0,
                format!(
                    "Using legacy config path {} — move it to {} so both runtimes agree.",
                    path.display(),
                    Paths::new(env).hub_home().display()
                ),
            );
        }
        Ok(loaded)
    }

    pub fn from_str(env: &Environment, text: &str) -> Result<Loaded, ConfigError> {
        let mut document: serde_yaml_ng::Value = serde_yaml_ng::from_str(text)?;
        let result = validate_document(&document);
        if !result.valid() {
            return Err(ConfigError::Validation { result });
        }
        resolve_env_vars_with(env, &mut document);
        let typed = type_document(&document)?;
        let notices = notices(env, &document, &typed.config);
        Ok(Loaded {
            config: typed.config,
            warnings: result.warnings,
            notices,
        })
    }
}

fn notices(env: &Environment, document: &serde_yaml_ng::Value, config: &Config) -> Vec<String> {
    let mut out = env.legacy_notices();
    out.extend(env.deprecation_warnings());
    out.extend(plugin_entry_notices(document));
    if config.pglite_import_pending() {
        out.push(PGLITE_IMPORT_NOTICE.to_owned());
    }
    out
}

fn plugin_entry_notices(document: &serde_yaml_ng::Value) -> Vec<String> {
    document
        .get("plugins")
        .and_then(serde_yaml_ng::Value::as_sequence)
        .into_iter()
        .flatten()
        .filter_map(serde_yaml_ng::Value::as_str)
        .filter(|entry| entry.starts_with("plugins/tools/"))
        .map(|entry| {
            format!(
                "{entry} is deprecated — built-in tools were removed; every harness ships its own."
            )
        })
        .collect()
}
