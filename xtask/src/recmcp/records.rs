use rmcp::model::{CallToolResult, JsonObject, ServerPeerInfo, Tool};
use serde::{Deserialize, Serialize};

pub const TOOLS_FILE: &str = "tools-list.json";
pub const CALLS_FILE: &str = "calls.json";
pub const META_FILE: &str = "meta.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsDocument {
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallsDocument {
    pub calls: Vec<CallRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallRecord {
    pub request: CallRequest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(flatten)]
    pub outcome: CallOutcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallRequest {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CallOutcome {
    Result { result: CallToolResult },
    ProtocolError { code: i32, message: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaDocument {
    pub recorder_version: String,
    pub program: String,
    pub recorded_at: String,
    pub server: ServerPeerInfo,
}
