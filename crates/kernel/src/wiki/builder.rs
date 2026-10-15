use async_trait::async_trait;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use wire::gateway::wiki::{WikiHistoryEntry, WikiSlug};

use crate::BoxError;

pub const WIKI_PIPELINE_VERSION: i32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtractWiki {
    pub summary_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
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
}

#[must_use]
pub fn extraction_schema() -> schemars::Schema {
    schemars::schema_for!(Vec<WikiPatch>)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WikiSummary {
    pub id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WikiWork {
    Pending(WikiSummary),
    Complete { topics: Vec<String> },
}

#[derive(Debug, Error)]
pub enum WikiStoreError {
    #[error("wiki store failed: {0}")]
    Query(#[source] BoxError),
    #[error("summary {0} is missing")]
    Missing(Uuid),
    #[error("conversation does not own summary {0}")]
    ConversationMismatch(Uuid),
    #[error("wiki slug conflicts with another topic: {0}")]
    SlugConflict(String),
    #[error("wiki update target is missing: {0}")]
    TopicMissing(String),
    #[error("invalid wiki patch: {0}")]
    InvalidPatch(String),
}

#[async_trait]
pub trait WikiBuilderStore: Send + Sync {
    async fn wiki_work(&self, job: &ExtractWiki) -> Result<WikiWork, WikiStoreError>;

    async fn wiki_backfill(&self, limit: u32) -> Result<Vec<ExtractWiki>, WikiStoreError>;

    async fn apply_wiki(
        &self,
        job: &ExtractWiki,
        patches: Vec<WikiPatch>,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, WikiStoreError>;

    async fn fail_wiki(
        &self,
        summary_id: Uuid,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError>;
}
