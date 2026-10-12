use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use thiserror::Error;
use wire::message::Role;
use wire::session_id::SessionId;

#[derive(Debug, Error)]
pub enum TranscriptReaderError {
    #[error("transcript store is unavailable: {0}")]
    Unavailable(String),
    #[error("transcript read failed: {0}")]
    Read(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub content: String,
    pub at: DateTime<Utc>,
}

#[async_trait]
pub trait TranscriptReader: Send + Sync {
    async fn sessions(
        &self,
        store: &std::path::Path,
    ) -> Result<Vec<SessionId>, TranscriptReaderError>;

    async fn read(&self, session: &SessionId) -> Result<Vec<Turn>, TranscriptReaderError>;

    async fn tail(
        &self,
        session: &SessionId,
        since: DateTime<Utc>,
    ) -> Result<Vec<Turn>, TranscriptReaderError>;
}
