use crate::BoxError;
use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use thiserror::Error;
use tokio::sync::mpsc::Receiver;
use wire::session_id::SessionId;

#[derive(Debug, Error)]
pub enum TerminalMuxError {
    #[error("terminal mux is unavailable")]
    Unavailable(#[source] BoxError),
    #[error("terminal is unknown: {0}")]
    UnknownTerminal(TerminalId),
    #[error("terminal exited: {0}")]
    Exited(TerminalId),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TerminalId(String);

impl TerminalId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TerminalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<TerminalId> for String {
    fn from(value: TerminalId) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyInfo {
    pub id: TerminalId,
    pub session: SessionId,
    pub running: bool,
}

#[async_trait]
pub trait TerminalMux: Send + Sync {
    async fn spawn(
        &self,
        session: &SessionId,
        argv: Vec<String>,
        cwd: &std::path::Path,
        size: TermSize,
    ) -> Result<TerminalId, TerminalMuxError>;

    async fn attach(&self, id: &TerminalId) -> Result<Receiver<Vec<u8>>, TerminalMuxError>;

    async fn input(&self, id: &TerminalId, bytes: Vec<u8>) -> Result<(), TerminalMuxError>;

    async fn resize(&self, id: &TerminalId, size: TermSize) -> Result<(), TerminalMuxError>;

    async fn kill(&self, id: &TerminalId) -> Result<(), TerminalMuxError>;

    async fn list(&self) -> Result<Vec<PtyInfo>, TerminalMuxError>;
}
