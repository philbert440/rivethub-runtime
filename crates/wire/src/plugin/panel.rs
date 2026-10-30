use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::JsonObject;
use crate::plugin::PluginId;
use crate::plugin::action::ViewAction;
use crate::plugin::canvas::ColorRole;
use crate::plugin::view::ViewContext;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PanelTheme {
    pub scheme: PanelScheme,
    pub colors: BTreeMap<ColorRole, String>,
    pub font_family: String,
    pub font_mono: String,
    pub font_size: f64,
    pub radius: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PanelScheme {
    Light,
    Dark,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum PanelHostMessage {
    #[serde(rename_all = "camelCase")]
    Init {
        theme: PanelTheme,
        params: Option<JsonObject>,
        context: ViewContext,
        plugin: PluginId,
        node: String,
    },
    #[serde(rename_all = "camelCase")]
    Theme { theme: PanelTheme },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum PanelMessage {
    Ready,
    #[serde(rename_all = "camelCase")]
    Resize {
        height: u32,
    },
    #[serde(rename_all = "camelCase")]
    Action {
        action: ViewAction,
    },
}
