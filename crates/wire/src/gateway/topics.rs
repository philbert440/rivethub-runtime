use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::gateway::wiki::WikiPageResponse;
use crate::gateway::wiki::WikiSlug;
use crate::session_id::SessionId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TopicDirectory {
    Local { node: String, path: String },
    Shared { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PlacedBy {
    You,
    Categorizer,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TopicRecord {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub aliases: Vec<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub directory: Option<TopicDirectory>,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub created_at: DateTime<Utc>,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub updated_at: DateTime<Utc>,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TopicSummary {
    #[serde(flatten)]
    pub topic: TopicRecord,
    pub session_count: u64,
    #[serde(default)]
    #[serde_as(as = "Option<serde_with::TimestampMilliSeconds>")]
    pub last_active: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TopicList {
    pub topics: Vec<TopicSummary>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TopicCreateRequest {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub directory: Option<TopicDirectory>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct TopicUpdate {
    pub name: Option<String>,
    pub aliases: Option<Vec<String>>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    #[schemars(with = "Option<Option<String>>")]
    pub parent: Option<Option<String>>,
    #[serde(default, with = "::serde_with::rust::double_option")]
    #[schemars(with = "Option<Option<TopicDirectory>>")]
    pub directory: Option<Option<TopicDirectory>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct TopicMergeRequest {
    pub into: String,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TopicPlacement {
    pub session_id: SessionId,
    pub topic_id: String,
    pub by: PlacedBy,
    #[serde_as(as = "serde_with::TimestampMilliSeconds")]
    pub at: DateTime<Utc>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TopicSuggestion {
    pub name: String,
    #[serde(default)]
    pub topic_id: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PlacementReply {
    #[serde(default)]
    pub placement: Option<TopicPlacement>,
    #[serde(default)]
    pub suggestion: Option<TopicSuggestion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PlacementRequest {
    pub topic_id: String,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TopicSession {
    #[serde(flatten)]
    pub placement: TopicPlacement,
    #[serde(default)]
    #[serde_as(as = "Option<serde_with::TimestampMilliSeconds>")]
    pub last_active: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TopicSessionList {
    pub sessions: Vec<TopicSession>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TopicPage {
    pub topic: TopicRecord,
    #[serde(default)]
    pub page: Option<WikiPageResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StartupKind {
    Topic,
    Recent,
    Relevant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StartupSource {
    Session(SessionId),
    Wiki(WikiSlug),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartupSection {
    pub kind: StartupKind,
    pub title: String,
    pub text: String,
    pub sources: Vec<StartupSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StartupContext {
    pub topic_id: String,
    pub sections: Vec<StartupSection>,
    pub tokens: usize,
}
