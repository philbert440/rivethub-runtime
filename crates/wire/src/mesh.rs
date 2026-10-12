use crate::{Extra, JsonObject};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshFile {
    pub version: i64,
    pub nodes: std::collections::BTreeMap<String, MeshNode>,
    pub updated_at: i64,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshRole {
    Agent,
    Datahub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshNodeStatus {
    Online,
    Offline,
    Degraded,
    Updating,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshNode {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<MeshRole>,
    pub agents: Vec<String>,
    pub host: String,
    pub port: u16,
    pub providers: Vec<String>,
    pub models: Vec<String>,
    pub capabilities: Vec<String>,
    pub status: MeshNodeStatus,
    pub last_seen: i64,
    pub registered_at: i64,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<JsonObject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshNodeEventType {
    NodeJoined,
    NodeLeft,
    NodeStale,
    NodeUpdated,
    NodeDegraded,
}
