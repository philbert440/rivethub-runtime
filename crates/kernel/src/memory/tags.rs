use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "snake_case")]
pub enum TagEntityType {
    Conversation,
    Summary,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        duptype,
        reason = "KERNELWIRE: the kernel owns a tag's review state; wire::gateway::memory::TagState is the contract's copy, translated by the gateway"
    )
)]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display, strum::EnumString,
)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "snake_case")]
pub enum TagState {
    Suggested,
    Accepted,
    Rejected,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagWire {
    pub id: String,
    pub entity_type: TagEntityType,
    pub entity_id: String,
    pub key: String,
    pub value: String,
    pub display: String,
    pub source: String,
    pub state: TagState,
    pub confidence: Option<f64>,
    pub proposed_by: String,
    pub reason: String,
    pub decided_by: Option<String>,
    pub decided_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingTagAction {
    Add,
    Remove,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingTagWire {
    #[serde(flatten)]
    pub tag: TagWire,
    pub action: Option<PendingTagAction>,
    pub session_key: Option<String>,
    pub title: Option<String>,
    pub agent: Option<String>,
    pub conversation_id: Option<String>,
    pub excerpt: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagTaxonomyEntry {
    pub key: String,
    pub value: String,
    pub display: String,
    pub parent_value: Option<String>,
    pub aliases: Vec<String>,
    pub state: TagState,
    pub source: String,
    pub reason: String,
    pub decided_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagTaxonomyMerge {
    pub key: String,
    pub from: String,
    pub into: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagTaxonomyMergeResult {
    pub moved: u64,
    pub dropped: u64,
    pub into: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagCount {
    pub key: String,
    pub value: String,
    pub display: String,
    pub conversations: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagEntryPair {
    pub key: String,
    pub value: String,
}
