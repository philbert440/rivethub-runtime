use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{WikiHistoryEntry, WikiSlug};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, strum::Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum WikiAction {
    Create,
    Update,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WikiPatch {
    pub action: WikiAction,
    pub slug: WikiSlug,
    pub title: String,
    pub aliases: Vec<WikiSlug>,
    pub tags: Vec<String>,
    pub entities: Vec<String>,
    pub summary: String,
    pub article: Option<String>,
    pub related: Option<Vec<WikiSlug>>,
    pub history_entry: Option<WikiHistoryEntry>,
    #[serde(rename = "type")]
    pub entity_type: Option<String>,
}
