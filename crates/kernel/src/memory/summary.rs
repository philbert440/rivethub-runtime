use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use thiserror::Error;
use uuid::Uuid;
use wire::message::Role;

use crate::BoxError;

pub const HEARTBEAT_SESSION_PREFIX: &str = "heartbeat:";

pub use wire::gateway::memory::SummaryKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationHead {
    pub id: Uuid,
    pub session_key: String,
    pub agent: String,
    pub channel: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowMessage {
    pub id: Uuid,
    pub agent: String,
    pub role: Role,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryNode {
    pub id: Uuid,
    pub kind: SummaryKind,
    pub content: String,
    pub message_count: u32,
    pub earliest_at: DateTime<Utc>,
    pub latest_at: DateTime<Utc>,
    pub depth: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationWindow {
    pub conversation: ConversationHead,
    pub messages: Vec<WindowMessage>,
    pub leaves: Vec<SummaryNode>,
    pub branches: Vec<SummaryNode>,
    pub latest_message_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummarySource {
    pub message_id: Uuid,
    pub ordinal: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSummary {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub kind: SummaryKind,
    pub depth: i32,
    pub content: String,
    pub message_count: u32,
    pub earliest_at: DateTime<Utc>,
    pub latest_at: DateTime<Utc>,
    pub model: String,
    pub pipeline_version: i32,
    pub created_at: DateTime<Utc>,
    pub sources: Vec<SummarySource>,
    pub child_ids: Vec<Uuid>,
}

#[derive(Debug, Error)]
pub enum SummaryStoreError {
    #[error("summary store query failed: {message}")]
    Query {
        message: String,
        #[source]
        source: BoxError,
    },
    #[error("summary {id} is missing")]
    Missing { id: String },
    #[error("stored summary kind {kind} is not leaf, branch, or root")]
    Kind { kind: String },
    #[error("a summary count does not fit")]
    Count,
    #[error("the summary store is closed")]
    Closed,
}

#[async_trait]
pub trait SummaryStore: Send + Sync {
    async fn conversation(&self, id: Uuid)
    -> Result<Option<ConversationWindow>, SummaryStoreError>;

    async fn pending(&self) -> Result<Vec<ConversationWindow>, SummaryStoreError>;

    async fn write_summary(&self, summary: NewSummary) -> Result<(), SummaryStoreError>;
}
