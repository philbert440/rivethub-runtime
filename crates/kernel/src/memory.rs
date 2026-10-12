use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use thiserror::Error;
use wire::gateway::memory::MemoryStatsResponse;
use wire::memory::MemoryEntry;
use wire::memory::MemorySearchResult;

#[derive(Debug, Error)]
pub enum MemoryStoreError {
    #[error("memory store is unavailable: {0}")]
    Unavailable(String),
    #[error("memory query failed: {0}")]
    Query(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    pub agent: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowseFilter {
    pub session_key: Option<String>,
    pub agent: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagAction {
    Accept,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagOutcome {
    pub moved: u64,
    pub dropped: u64,
}

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn append(&self, batch: Vec<MemoryEntry>) -> Result<u64, MemoryStoreError>;

    async fn search(&self, query: SearchQuery)
    -> Result<Vec<MemorySearchResult>, MemoryStoreError>;

    async fn browse(&self, filter: BrowseFilter) -> Result<Vec<MemoryEntry>, MemoryStoreError>;

    async fn stats(&self) -> Result<MemoryStatsResponse, MemoryStoreError>;

    async fn get_full(&self, id: &str) -> Result<Option<MemoryEntry>, MemoryStoreError>;

    async fn tags(&self, action: TagAction) -> Result<TagOutcome, MemoryStoreError>;

    async fn context(&self, session: &str, now: DateTime<Utc>) -> Result<String, MemoryStoreError>;
}
