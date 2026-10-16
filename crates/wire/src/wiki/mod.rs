mod markdown;
mod patch;

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::Extra;
use crate::gateway::wiki::{WikiHistoryEntry, WikiSlug, WikiSource};

pub use markdown::{parse_wiki_page, serialize_wiki_page};
pub use patch::{WikiAction, WikiPatch, apply_patch, build_wiki_search_text, extract_wiki_links};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WikiFrontmatter {
    pub title: String,
    pub slug: WikiSlug,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_verified: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<WikiSource>,
    #[serde(flatten, default)]
    pub extra: Extra,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WikiPage {
    pub frontmatter: WikiFrontmatter,
    pub preamble: String,
    pub summary: Option<String>,
    pub article: Option<String>,
    pub see_also: Option<String>,
    pub history: Vec<WikiHistoryEntry>,
    pub citations: Vec<PageCitation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageCitation {
    pub summary_id: String,
    pub date: NaiveDate,
    pub kind: CitationKind,
    pub note: String,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::EnumString, strum::Display,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum CitationKind {
    Leaf,
    Branch,
    Root,
    Condensed,
}

#[derive(Debug, thiserror::Error)]
pub enum WikiPageError {
    #[error("wiki frontmatter: {0}")]
    Frontmatter(#[from] serde_yaml_ng::Error),
    #[error("invalid wiki page: {0}")]
    Format(String),
    #[error("invalid wiki date: {0}")]
    Date(#[from] chrono::ParseError),
    #[error("invalid wiki citation kind: {0}")]
    CitationKind(#[from] strum::ParseError),
}
