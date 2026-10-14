use crate::Extra;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::Serialize;
use serde_with::TryFromInto;
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
    pub metadata: Option<MeshMetadata>,
    pub ssh_user: Option<String>,
    pub install_root: Option<String>,
    pub platform: Option<String>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MeshMetadata {
    #[serde(default, rename = "denUrl")]
    pub gateway_url: Option<url::Url>,
    #[serde(default, rename = "denPort")]
    #[serde_as(as = "Option<TryFromInto<CutoverPort>>")]
    pub gateway_port: Option<u16>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CutoverPortError {
    #[error("mesh gateway port is not a decimal integer")]
    NotDecimal,
    #[error("mesh gateway port is outside the range of a port")]
    OutOfRange,
}

pub fn parse_cutover_gateway_port(text: &str) -> Result<u16, CutoverPortError> {
    if !is_cutover_decimal(text) {
        return Err(CutoverPortError::NotDecimal);
    }
    text.parse::<u16>()
        .map_err(|_| CutoverPortError::OutOfRange)
}

fn is_cutover_decimal(text: &str) -> bool {
    let mut bytes = text.bytes();
    match bytes.next() {
        Some(b'0') => bytes.next().is_none(),
        Some(b'1'..=b'9') => bytes.all(|byte| byte.is_ascii_digit()),
        Some(_) | None => false,
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum CutoverPort {
    Number(u16),
    Decimal(String),
}

impl From<u16> for CutoverPort {
    fn from(port: u16) -> Self {
        Self::Number(port)
    }
}

impl TryFrom<CutoverPort> for u16 {
    type Error = CutoverPortError;

    fn try_from(port: CutoverPort) -> Result<Self, Self::Error> {
        match port {
            CutoverPort::Number(port) => Ok(port),
            CutoverPort::Decimal(text) => parse_cutover_gateway_port(&text),
        }
    }
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

impl std::fmt::Display for MeshParseCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidShape => "MESH_INVALID_SHAPE",
            Self::FlatArray => "MESH_FLAT_ARRAY",
            Self::JsonInvalid => "MESH_JSON_INVALID",
        })
    }
}
