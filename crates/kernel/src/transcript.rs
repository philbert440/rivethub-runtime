use crate::BoxError;
use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use thiserror::Error;
use wire::gateway::sessions::HarnessToolOutput;
use wire::gateway::sessions::HarnessTranscriptTurn;
use wire::harness::HarnessId;
use wire::harness::HarnessSessionStatus;
use wire::harness::HarnessSessionSummary;
use wire::harness::HarnessStoreSession;
use wire::harness::RosterCommand;
use wire::message::Role;
use wire::session_id::SessionId;

#[derive(Debug, Error)]
pub enum TranscriptReaderError {
    #[error("transcript store is unavailable")]
    Unavailable(#[source] BoxError),
    #[error("transcript read failed: {message}")]
    Read {
        message: String,
        #[source]
        source: BoxError,
    },
}

#[derive(Debug, Error)]
pub enum SessionRenameError {
    #[error("{0} sessions cannot be renamed")]
    Unsupported(HarnessId),
    #[error("the session has no record in its harness store")]
    Missing,
    #[error("writing the session title failed: {message}")]
    Write {
        message: String,
        #[source]
        source: BoxError,
    },
    #[error(transparent)]
    Store(#[from] TranscriptReaderError),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreSession {
    pub harness: HarnessId,
    pub id: String,
    pub title: String,
    pub updated_at: i64,
    pub created_at: Option<i64>,
    pub cwd: Option<String>,
    pub parent_session_id: Option<String>,
    pub agent_name: Option<String>,
    pub model: Option<String>,
    pub task_id: Option<String>,
}

impl StoreSession {
    #[must_use]
    pub fn new(harness: HarnessId, id: &str, updated_at: i64) -> Self {
        Self {
            harness,
            id: id.to_owned(),
            title: id.to_owned(),
            updated_at,
            created_at: None,
            cwd: None,
            parent_session_id: None,
            agent_name: None,
            model: None,
            task_id: None,
        }
    }

    #[must_use]
    pub fn legacy_row(&self) -> HarnessStoreSession {
        HarnessStoreSession {
            id: self.id.clone(),
            command: RosterCommand::from(self.harness),
            title: self.title.clone(),
            updated_at: epoch_ms(self.updated_at),
            created_at: self.created_at.map(epoch_ms),
            parent_session_id: self.parent_session_id.clone(),
            agent_name: self.agent_name.clone(),
            model: self.model.clone(),
            task_id: self.task_id.clone(),
        }
    }

    #[must_use]
    pub fn summary(&self, cwd: Option<String>, now: DateTime<Utc>) -> HarnessSessionSummary {
        let updated_at = if self.updated_at == 0 {
            now
        } else {
            epoch_ms(self.updated_at)
        };
        HarnessSessionSummary {
            session_id: SessionId::new(self.harness, &self.id),
            harness_id: self.harness,
            title: (self.title != self.id).then(|| self.title.clone()),
            cwd,
            created_at: epoch_ms(self.created_at.unwrap_or(self.updated_at)),
            updated_at,
            status: HarnessSessionStatus::Ended,
            blocked: None,
            supersedes: None,
            model: self.model.clone(),
            parent_session_id: self
                .parent_session_id
                .as_deref()
                .and_then(|parent| linked_parent(self.harness, parent)),
            agent_name: self.agent_name.clone(),
            task_id: self.task_id.clone(),
            effort: None,
            transport: None,
            extra: wire::Extra::default(),
        }
    }
}

fn epoch_ms(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or(DateTime::UNIX_EPOCH)
}

fn linked_parent(harness: HarnessId, parent: &str) -> Option<SessionId> {
    let linkable = !parent.is_empty()
        && !parent.contains('/')
        && !parent.contains("..")
        && !parent.starts_with(char::is_whitespace)
        && !parent.ends_with(char::is_whitespace);
    linkable.then(|| SessionId::new(harness, parent))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListLimit(usize);

impl ListLimit {
    pub const DEFAULT: Self = Self(100);
    pub const MAX: usize = 500;

    #[must_use]
    pub const fn per_harness_route() -> Self {
        Self::DEFAULT
    }

    #[must_use]
    pub fn from_query(raw: Option<&str>) -> Self {
        match raw.and_then(LeadingInteger::parse) {
            Some(LeadingInteger::Positive(value)) if value > 0 => Self(value.min(Self::MAX)),
            Some(LeadingInteger::Positive(_) | LeadingInteger::NonPositive) | None => Self::DEFAULT,
        }
    }

    #[must_use]
    pub const fn get(self) -> usize {
        self.0
    }
}

enum LeadingInteger {
    Positive(usize),
    NonPositive,
}

impl LeadingInteger {
    fn parse(raw: &str) -> Option<Self> {
        let text = raw.trim_start();
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix('+').unwrap_or(text)),
        };
        let run: Vec<u8> = digits
            .bytes()
            .take_while(u8::is_ascii_digit)
            .map(|digit| digit - b'0')
            .collect();
        if run.is_empty() {
            return None;
        }
        let magnitude = run.iter().fold(0_usize, |total, digit| {
            total.saturating_mul(10).saturating_add(usize::from(*digit))
        });
        Some(if negative {
            Self::NonPositive
        } else {
            Self::Positive(magnitude)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoreTranscript {
    pub turns: Vec<HarnessTranscriptTurn>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptRead {
    Read(StoreTranscript),
    NoSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolOutputRead {
    Found(HarnessToolOutput),
    Missing,
    NoSource,
}

#[async_trait]
pub trait SessionLister: Send + Sync {
    async fn list(&self, harnesses: &[HarnessId], limit: ListLimit) -> Vec<StoreSession>;

    async fn describe(
        &self,
        harness: HarnessId,
        native: &str,
        parent: Option<&str>,
    ) -> Result<Option<StoreSession>, TranscriptReaderError>;

    async fn store_cwd(
        &self,
        harness: HarnessId,
        native: &str,
    ) -> Result<Option<String>, TranscriptReaderError>;

    async fn transcript(
        &self,
        harness: HarnessId,
        native: &str,
    ) -> Result<TranscriptRead, TranscriptReaderError>;

    async fn rename(
        &self,
        harness: HarnessId,
        native: &str,
        title: &str,
    ) -> Result<StoreSession, SessionRenameError>;

    async fn tool_output(
        &self,
        harness: HarnessId,
        native: &str,
        tool_call_id: &str,
    ) -> Result<ToolOutputRead, TranscriptReaderError>;
}
