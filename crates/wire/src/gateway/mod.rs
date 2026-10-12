pub mod agents;
pub mod catalog;
pub mod devices;
pub mod memory;
pub mod sessions;
pub mod term;
pub mod users;
pub mod wiki;

use crate::harness::HarnessId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthzResponse {
    pub ok: bool,
    pub sessions: u64,
    pub name: String,
    pub node: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayError {
    pub error: String,
    pub detail: Option<String>,
    pub hint: Option<String>,
    pub code: Option<String>,
    pub harness_id: Option<HarnessId>,
    pub node: Option<String>,
    pub retryable: Option<bool>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OkReply {
    pub ok: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsIngested {
    pub ok: bool,
    pub ingested: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRedeemRequest {
    pub token: String,
}
