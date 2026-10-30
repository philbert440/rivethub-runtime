use crate::harness::HarnessId;
use serde::{Deserialize, Serialize};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename = "preset", rename_all = "camelCase")]
pub struct CatalogPreset {
    pub id: String,
    pub name: String,
    pub node: String,
    pub local: bool,
    #[serde(default)]
    pub harness_id: Option<HarnessId>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(default)]
    pub implemented: Option<bool>,
    #[serde(default)]
    pub gap: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogNode {
    pub id: String,
    pub node: String,
    pub local: bool,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum CatalogAgent {
    Preset(CatalogPreset),
    Node(CatalogNode),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAgentsResponse {
    pub agents: Vec<CatalogAgent>,
}
