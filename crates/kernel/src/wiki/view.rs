use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use values::SessionId;

use super::page::{WikiHistoryEntry, WikiSourceKind};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiTopicList {
    pub topics: Vec<WikiTopic>,
    pub total: u64,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiTopic {
    pub slug: String,
    pub title: String,
    pub tags: Vec<String>,
    pub entities: Vec<String>,
    #[serde_as(as = "values::Millis")]
    pub updated_at: DateTime<Utc>,
    pub excerpt: String,
    #[serde(default)]
    pub topic_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiGapsResponse {
    pub red_links: Vec<WikiRedLink>,
    pub stalest: Vec<WikiTopic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiRedLink {
    pub entity: String,
    pub referenced_by: Vec<String>,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiPageResponse {
    pub slug: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub entities: Vec<String>,
    pub current_state: String,
    pub article: Option<String>,
    pub see_also: Option<Vec<String>>,
    pub history: Vec<WikiHistoryEntry>,
    pub citations: Vec<WikiCitation>,
    pub markdown: String,
    pub sources: Vec<WikiSource>,
    pub git_sha: Option<String>,
    #[serde(default)]
    #[serde_as(as = "Option<values::Millis>")]
    pub last_verified: Option<DateTime<Utc>>,
    #[serde_as(as = "values::Millis")]
    pub updated_at: DateTime<Utc>,
    pub related: Option<Vec<String>>,
    #[serde(default)]
    pub topic_id: Option<String>,
}

#[serde_as]
#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiCitation {
    pub summary_id: String,
    #[serde(default)]
    #[serde_as(as = "Option<values::Millis>")]
    pub date: Option<DateTime<Utc>>,
    pub kind: Option<String>,
    pub note: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub topic_id: Option<String>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiSource {
    pub kind: WikiSourceKind,
    pub ids: Vec<String>,
    pub conversation_id: Option<String>,
    pub span: Option<WikiSourceSpan>,
}

#[serde_as]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiSourceSpan {
    #[serde_as(as = "values::Millis")]
    pub earliest: DateTime<Utc>,
    #[serde_as(as = "values::Millis")]
    pub latest: DateTime<Utc>,
}
