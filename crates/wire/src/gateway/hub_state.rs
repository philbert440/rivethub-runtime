use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::events::ProtocolVersion;
use crate::harness::HarnessId;
use crate::stream::ThinkingLevel;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubProjectDefaults {
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<ThinkingLevel>,
    #[serde(default)]
    pub harness_id: Option<HarnessId>,
    #[serde(default)]
    pub node: Option<String>,
    #[serde(default)]
    pub directory: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubProject {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub order: f64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub defaults: Option<HubProjectDefaults>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubLastSession {
    pub key: String,
    pub at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum HubSessionMode {
    Chat,
    Terminal,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubState {
    #[serde(default)]
    pub projects: Vec<HubProject>,
    #[serde(default)]
    pub default_project_id: Option<String>,
    #[serde(default)]
    pub membership: BTreeMap<String, String>,
    #[serde(default)]
    pub names: BTreeMap<String, String>,
    #[serde(default)]
    pub archived: Vec<String>,
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default)]
    pub hidden: Vec<String>,
    #[serde(default)]
    pub modes: BTreeMap<String, HubSessionMode>,
    #[serde(default)]
    pub last_session: Option<HubLastSession>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubStateResponse {
    pub version: u64,
    pub state: HubState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HubStatePutRequest {
    pub base_version: u64,
    pub state: HubState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename = "hub.state", rename_all = "camelCase")]
pub struct HubStateChanged {
    pub v: ProtocolVersion,
    pub version: u64,
}
