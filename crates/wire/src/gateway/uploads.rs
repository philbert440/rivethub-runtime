use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadCreated {
    pub id: String,
    pub uri: String,
    pub name: String,
    pub bytes: u64,
    pub media_type: String,
    pub expires_at: DateTime<Utc>,
    pub session: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UploadList {
    pub uploads: Vec<UploadCreated>,
}
