use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiHistoryEntry {
    pub date: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiCitation {
    pub summary_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiSource {
    pub kind: String,
    pub ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<WikiSourceSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiSourceSpan {
    pub earliest: String,
    pub latest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiPageResponse {
    pub slug: String,
    pub title: String,
    pub aliases: Vec<String>,
    pub tags: Vec<String>,
    pub entities: Vec<String>,
    pub current_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub article: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub see_also: Option<Vec<String>>,
    pub history: Vec<WikiHistoryEntry>,
    pub citations: Vec<WikiCitation>,
    pub markdown: String,
    pub sources: Vec<WikiSource>,
    #[serde(default)]
    pub git_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_verified: Option<String>,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiMissSuggestion {
    pub slug: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiMissBody {
    pub error: String,
    pub suggestions: Vec<WikiMissSuggestion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiRedLink {
    pub entity: String,
    pub referenced_by: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiGapsResponse {
    pub red_links: Vec<WikiRedLink>,
    pub stalest: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiTopic {
    pub slug: String,
    pub title: String,
    pub tags: Vec<String>,
    pub entities: Vec<String>,
    pub updated_at: String,
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiTopicList {
    pub topics: Vec<WikiTopic>,
    pub total: u64,
}
