pub mod action;
pub mod canvas;
pub mod host;
pub mod manifest;
pub mod panel;
pub mod process;
pub mod view;

use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(extend("pattern" = "^[a-z](-?[a-z0-9])+$", "maxLength" = 24))]
pub struct PluginId(String);

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(extend("pattern" = "^[a-z](_?[a-z0-9])*$", "maxLength" = 24))]
pub struct ToolName(String);

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(extend("pattern" = "^[a-z0-9][a-z0-9_-]*$", "maxLength" = 64))]
pub struct RouteName(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PluginNameError {
    #[error(
        "a plug-in id is 2 to 24 characters of a-z, 0-9 and single hyphens, starting with a letter and not ending with a hyphen"
    )]
    PluginId,
    #[error(
        "a tool name is 1 to 24 characters of a-z, 0-9 and single underscores, starting with a letter and not ending with an underscore"
    )]
    ToolName,
    #[error(
        "a name is 1 to 64 characters of a-z, 0-9, hyphens and underscores, starting with a letter or digit"
    )]
    RouteName,
}

fn separated(text: &str, min: usize, separator: char, doubled: &str) -> bool {
    (min..=24).contains(&text.len())
        && text.starts_with(|first: char| first.is_ascii_lowercase())
        && !text.ends_with(separator)
        && !text.contains(doubled)
        && text
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == separator)
}

fn route_name(text: &str) -> bool {
    (1..=64).contains(&text.len())
        && text.starts_with(|first: char| first.is_ascii_lowercase() || first.is_ascii_digit())
        && text
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
}

impl TryFrom<String> for PluginId {
    type Error = PluginNameError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        if separated(&id, 2, '-', "--") {
            Ok(Self(id))
        } else {
            Err(PluginNameError::PluginId)
        }
    }
}

impl TryFrom<String> for ToolName {
    type Error = PluginNameError;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        if separated(&name, 1, '_', "__") {
            Ok(Self(name))
        } else {
            Err(PluginNameError::ToolName)
        }
    }
}

impl TryFrom<String> for RouteName {
    type Error = PluginNameError;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        if route_name(&name) {
            Ok(Self(name))
        } else {
            Err(PluginNameError::RouteName)
        }
    }
}

impl From<PluginId> for String {
    fn from(id: PluginId) -> Self {
        id.0
    }
}

impl From<ToolName> for String {
    fn from(name: ToolName) -> Self {
        name.0
    }
}

impl From<RouteName> for String {
    fn from(name: RouteName) -> Self {
        name.0
    }
}

impl AsRef<str> for PluginId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ToolName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for RouteName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
