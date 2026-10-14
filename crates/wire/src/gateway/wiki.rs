use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WikiQuery {
    pub q: Option<String>,
    pub tag: Option<String>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WikiPageQuery {
    pub slug: WikiSlug,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct WikiSlug(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid wiki slug")]
pub struct WikiSlugError;

impl TryFrom<String> for WikiSlug {
    type Error = WikiSlugError;

    fn try_from(slug: String) -> Result<Self, Self::Error> {
        if slug.trim().is_empty()
            || matches!(slug.as_str(), "." | "..")
            || slug
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
        {
            return Err(WikiSlugError);
        }
        Ok(Self(slug))
    }
}

impl AsRef<str> for WikiSlug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<WikiSlug> for String {
    fn from(slug: WikiSlug) -> Self {
        slug.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiHistoryEntry {
    pub date: String,
    pub title: String,
    pub body: String,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiCitation {
    pub summary_id: String,
    pub date: Option<String>,
    pub kind: Option<String>,
    pub note: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum WikiSourceKind {
    Summary,
    Message,
    Conversation,
    Task,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WikiSourceSpan {
    pub earliest: String,
    pub latest: String,
}

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
    pub last_verified: Option<String>,
    pub updated_at: String,
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
