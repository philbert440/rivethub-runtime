use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::SummaryKind;

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemorySummaryNode {
    pub id: String,
    pub parent_id: Option<String>,
    pub level: u32,
    pub kind: SummaryKind,
    pub content: String,
    pub message_count: u32,
    pub earliest_at: Option<DateTime<Utc>>,
    pub latest_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub source_message_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemorySummaryDagResponse {
    pub session_id: String,
    pub summaries: Vec<MemorySummaryNode>,
}
