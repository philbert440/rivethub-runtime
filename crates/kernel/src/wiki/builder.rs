use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use wire::gateway::wiki::WikiSlug;

use crate::BoxError;
use crate::filing::Target;
use crate::topics::Slug;
use crate::topics::TopicId;

pub const WIKI_PIPELINE_VERSION: i32 = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtractWiki {
    pub summary_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<Uuid>,
}

pub use wire::wiki::{WikiAction, WikiPatch};

#[must_use]
pub fn extraction_schema() -> schemars::Schema {
    schemars::schema_for!(Vec<WikiPatch>)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WikiSummary {
    pub id: Uuid,
    pub conversation_id: Option<Uuid>,
    pub content: String,
    #[serde(skip)]
    pub session_key: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WikiRouting {
    pub topic_page: Option<WikiSlug>,
    pub cited: Vec<WikiSlug>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldEntity {
    pub key: Slug,
    pub source: String,
    pub summary_id: Uuid,
    pub patch: WikiPatch,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredWiki {
    pub summary_id: Uuid,
    pub waits: Target,
    pub applied: Option<TopicId>,
    pub patches: Vec<WikiPatch>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WikiWork {
    Pending(WikiSummary),
    Complete { topics: Vec<String> },
}

#[derive(Debug, Error)]
pub enum WikiStoreError {
    #[error("wiki directory is not configured")]
    DirectoryMissing,
    #[error("wiki page write failed: {0}")]
    Page(#[from] WikiWriteError),
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
        routing: &WikiRouting,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, WikiStoreError>;

    async fn fail_wiki(
        &self,
        summary_id: Uuid,
        error: &str,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError>;

    async fn hold_entity(&self, held: HeldEntity) -> Result<Vec<HeldEntity>, WikiStoreError>;

    async fn release_entity(&self, key: &Slug) -> Result<(), WikiStoreError>;

    async fn defer_wiki(&self, deferred: DeferredWiki) -> Result<(), WikiStoreError>;

    async fn unsettled_notes(&self, limit: u32) -> Result<Vec<DeferredWiki>, WikiStoreError>;

    async fn cite_wiki(
        &self,
        summary_id: Uuid,
        slug: &WikiSlug,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError>;
}

#[derive(Debug, Error)]
pub enum WikiWriteError {
    #[error("wiki filesystem: {0}")]
    Io(#[from] std::io::Error),
    #[error("wiki page: {0}")]
    Page(#[from] wire::wiki::WikiPageError),
    #[error("wiki writer lock exhausted 200 attempts")]
    LockTimeout,
    #[error("wiki path escapes its repository: {0}")]
    Containment(std::path::PathBuf),
    #[error("wiki git {operation} failed: {message}")]
    Git { operation: String, message: String },
    #[error("wiki git output is not UTF-8: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),
    #[error("wiki writer task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("wiki page slug does not match its file")]
    SlugMismatch,
    #[error("wiki page slug: {0}")]
    Slug(#[from] wire::gateway::wiki::WikiSlugError),
}
