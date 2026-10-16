use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Format(#[from] std::fmt::Error),
    #[error("invalid arguments: {0}")]
    Arguments(#[from] serde_json::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("operation refused: {0}")]
    Refused(&'static str),
    #[error("operation cancelled")]
    Cancelled,
    #[error(transparent)]
    Memory(#[from] kernel::memory::MemoryStoreError),
    #[error(transparent)]
    WikiSlug(#[from] wire::gateway::wiki::WikiSlugError),
    #[error("wiki page not found: {0}")]
    WikiMissing(String),
    #[error("wiki redirect cycle or chain exceeds 16 lookups")]
    WikiRedirect,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Unix socket is listening at {path}")]
    SocketListening { path: std::path::PathBuf },
    #[error("Unix socket path is not a socket: {path}")]
    SocketNotSocket { path: std::path::PathBuf },
    #[error("Unix socket path changed during startup: {path}")]
    SocketChanged { path: std::path::PathBuf },
    #[error("Unix socket operation failed at {path}: {source}")]
    SocketIo {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("Unix socket liveness check timed out at {path}")]
    SocketProbeTimeout { path: std::path::PathBuf },
    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
    #[error("MCP initialization failed: {0}")]
    Initialize(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error(transparent)]
    Service(#[from] rmcp::service::ServiceError),
}

pub fn required<T>(value: Option<T>, field: &str) -> Result<T, Error> {
    value.ok_or_else(|| Error::Invalid(format!("{field} is required")))
}
