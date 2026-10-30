use serde::{Deserialize, Serialize};

use crate::json::JsonObject;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum SessionDriver {
    Chat,
    Terminal,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TerminalApproval {
    pub tool: String,
    pub input: Option<JsonObject>,
}
