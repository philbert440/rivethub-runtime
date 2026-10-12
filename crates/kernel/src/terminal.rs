use async_trait::async_trait;
use thiserror::Error;
use tokio::sync::mpsc::Receiver;
use wire::session_id::SessionId;

#[derive(Debug, Error)]
pub enum TerminalMuxError {
    #[error("terminal mux is unavailable: {0}")]
    Unavailable(String),
    #[error("terminal is unknown: {0}")]
    UnknownTerminal(String),
    #[error("terminal exited: {0}")]
    Exited(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtyInfo {
    pub id: String,
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
    ) -> Result<String, TerminalMuxError>;

    async fn attach(&self, id: &str) -> Result<Receiver<Vec<u8>>, TerminalMuxError>;

    async fn input(&self, id: &str, bytes: Vec<u8>) -> Result<(), TerminalMuxError>;

    async fn resize(&self, id: &str, size: TermSize) -> Result<(), TerminalMuxError>;

    async fn kill(&self, id: &str) -> Result<(), TerminalMuxError>;

    async fn list(&self) -> Result<Vec<PtyInfo>, TerminalMuxError>;
}
