use crate::harness::HarnessId;
use crate::task::HarnessExecutorCapabilities;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename = "preset", rename_all = "camelCase")]
pub struct CatalogPreset {
    pub id: String,
    pub name: String,
    pub node: String,
    pub local: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_id: Option<HarnessId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implemented: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogNode {
    pub id: String,
    pub node: String,
    pub local: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CatalogAgent {
    Preset(CatalogPreset),
    Node(CatalogNode),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCommand {
    pub name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg_hint: Option<String>,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogExecutorEntry {
    pub key: String,
    pub capabilities: HarnessExecutorCapabilities,
    pub commands: Vec<CatalogCommand>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_id: Option<HarnessId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub implemented: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSkillEntry {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSheet {
    pub node: String,
    pub agents: Vec<CatalogAgent>,
    pub executors: Vec<CatalogExecutorEntry>,
    pub tools: Vec<String>,
    pub skills: Vec<CatalogSkillEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogAgentsResponse {
    pub agents: Vec<CatalogAgent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutcomesFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutcomesResponse {
    pub filter: OutcomesFilter,
    pub totals: OutcomeBucket,
    pub by_agent: std::collections::BTreeMap<String, OutcomeBucket>,
    pub by_executor: std::collections::BTreeMap<String, OutcomeBucket>,
    pub by_day: std::collections::BTreeMap<String, OutcomeBucket>,
}
