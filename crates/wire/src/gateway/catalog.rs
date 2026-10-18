use crate::harness::HarnessId;
use crate::task::HarnessExecutorCapabilities;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCommand {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub arg_hint: Option<String>,
    pub source: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogExecutorEntry {
    pub key: String,
    pub capabilities: HarnessExecutorCapabilities,
    pub commands: Vec<CatalogCommand>,
    #[serde(default)]
    pub harness_id: Option<HarnessId>,
    #[serde(default)]
    pub implemented: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSkillEntry {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSheet {
    pub node: String,
    pub agents: Vec<CatalogAgent>,
    pub executors: Vec<CatalogExecutorEntry>,
    pub tools: Vec<String>,
    pub skills: Vec<CatalogSkillEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAgentsResponse {
    pub agents: Vec<CatalogAgent>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeBucket {
    pub tasks: u64,
    pub completed: u64,
    pub failed: u64,
    pub verified: u64,
    pub refuted: u64,
    pub escalated: u64,
    pub diverged: u64,
    pub divergence_rate: f64,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub avg_duration_ms: Option<u64>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct OutcomesFilter {
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    pub agent_id: Option<String>,
    pub origin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OutcomesResponse {
    pub filter: OutcomesFilter,
    pub totals: OutcomeBucket,
    pub by_agent: BTreeMap<String, OutcomeBucket>,
    pub by_executor: BTreeMap<String, OutcomeBucket>,
    pub by_day: BTreeMap<String, OutcomeBucket>,
}
