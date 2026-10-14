use crate::{Extra, JsonObject};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshFile {
    pub version: u32,
    pub nodes: std::collections::BTreeMap<String, MeshNode>,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub updated_at: DateTime<Utc>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

fn registry_version() -> u32 {
    1
}

impl MeshFile {
    pub fn empty(now: DateTime<Utc>) -> Self {
        Self {
            version: registry_version(),
            nodes: std::collections::BTreeMap::new(),
            updated_at: now,
            extra: Extra::default(),
        }
    }
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

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshNode {
    pub id: String,
    pub name: String,
    pub role: Option<MeshRole>,
    pub agents: Vec<String>,
    pub host: String,
    pub port: u16,
    pub providers: Vec<String>,
    pub models: Vec<String>,
    pub capabilities: Vec<String>,
    pub status: MeshNodeStatus,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub last_seen: DateTime<Utc>,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub registered_at: DateTime<Utc>,
    pub version: String,
    pub metadata: Option<JsonObject>,
    pub ssh_user: Option<String>,
    pub install_root: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshParseCode {
    #[serde(rename = "MESH_INVALID_SHAPE")]
    InvalidShape,
    #[serde(rename = "MESH_FLAT_ARRAY")]
    FlatArray,
    #[serde(rename = "MESH_JSON_INVALID")]
    JsonInvalid,
}
