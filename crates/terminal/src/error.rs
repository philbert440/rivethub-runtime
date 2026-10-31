use kernel::terminal::TerminalId;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("herdr terminals stream through the herdr attach client")]
    HerdrStream,
    #[error("herdr terminals have no manager-visible exit")]
    HerdrExit,
    #[error("herdr server record missing")]
    MissingServer,
    #[error("herdr requires a terminal session key")]
    MissingSession,
    #[error("herdr socket has no session directory")]
    MissingSessionDirectory,
    #[error("terminal manager is shutting down")]
    ShuttingDown,
    #[error("terminal environment name is not UTF-8")]
    EnvironmentName,
    #[error("terminal argv is empty")]
    EmptyArgv,
    #[error("terminal PTY operation failed: {0}")]
    PtyBackend(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("pid {0} does not fit a signal target")]
    InvalidPid(u32),
    #[error("{signal} to {pid} failed: {source}")]
    Signal {
        pid: u32,
        signal: nix::sys::signal::Signal,
        #[source]
        source: nix::errno::Errno,
    },
    #[error("herdr server stop exited with {0}")]
    StopStatus(std::process::ExitStatus),
    #[error("socket path too long ({len} > 107): {path}")]
    SocketPathTooLong {
        len: usize,
        path: Box<std::path::Path>,
    },
    #[error("herdr server did not open {socket} within {timeout_ms}ms for {session}")]
    HerdrServerTimeout {
        socket: Box<std::path::Path>,
        timeout_ms: u128,
        session: String,
    },
    #[error("herdr server for {session} exited ({status}) before opening its socket: {said}")]
    HerdrServerExited {
        session: String,
        status: std::process::ExitStatus,
        said: String,
    },
    #[error("herdr server did not release {socket} within {timeout_ms}ms for {session}")]
    HerdrStopTimeout {
        socket: Box<std::path::Path>,
        timeout_ms: u128,
        session: String,
    },
    #[error("herdr rpc {method}: {error}")]
    Ack {
        method: crate::herdr::codec::RpcMethod,
        error: crate::herdr::codec::AckError,
    },
    #[error("herdr JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("herdr rpc {method} timed out after {timeout_ms}ms")]
    RpcTimeout {
        method: crate::herdr::codec::RpcMethod,
        timeout_ms: u128,
    },
    #[error("herdr rpc {method}: socket closed")]
    RpcClosed {
        method: crate::herdr::codec::RpcMethod,
    },
    #[error("herdr rpc: unexpected acknowledgement {actual}, expected {expected}")]
    AckId { expected: String, actual: String },
    #[error("herdr rpc: expected subscription_started")]
    SubscriptionResult,
    #[error(transparent)]
    Probe(std::sync::Arc<crate::herdr::ProbeFailure>),
    #[error("terminal worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
    #[error("terminal worker channel closed: {0}")]
    WorkerClosed(#[from] tokio::sync::oneshot::error::RecvError),
    #[error("terminal lifecycle channel closed: {0}")]
    LifecycleClosed(#[from] tokio::sync::watch::error::RecvError),
    #[error("terminal failed: {0}")]
    PtyFailure(#[source] std::sync::Arc<Self>),
    #[error("terminal {terminal} failed")]
    Failed { terminal: TerminalId },
    #[error("pty limit reached ({max}); all active")]
    PtyLimit { max: usize },
    #[error("terminal {terminal} is unknown")]
    UnknownTerminal { terminal: TerminalId },
    #[error("terminal {terminal} has exited")]
    Exited { terminal: TerminalId },
    #[error("session key {0} is not a terminal session key")]
    InvalidSessionKey(String),
    #[error("roster key {0} is not a terminal roster key")]
    InvalidRosterKey(String),
    #[error("terminal io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("herdr namespace {path} is outside {root}")]
    NamespaceEscape {
        path: Box<std::path::Path>,
        root: Box<std::path::Path>,
    },
    #[error("terminal namespace owner is unset")]
    OwnerUnset,
    #[error("herdr session stamp is invalid: {0}")]
    SessionStamp(#[source] serde_json::Error),
    #[error(transparent)]
    Roster(#[from] crate::roster::RosterError),
}

impl From<Error> for kernel::terminal::TerminalMuxError {
    fn from(value: Error) -> Self {
        match value {
            Error::UnknownTerminal { terminal } => Self::UnknownTerminal(terminal),
            Error::Exited { terminal } => Self::Exited(terminal),
            other @ (Error::HerdrStream
            | Error::HerdrExit
            | Error::MissingServer
            | Error::MissingSession
            | Error::MissingSessionDirectory
            | Error::ShuttingDown
            | Error::EnvironmentName
            | Error::EmptyArgv
            | Error::PtyBackend(_)
            | Error::InvalidPid(_)
            | Error::Signal { .. }
            | Error::StopStatus(_)
            | Error::SocketPathTooLong { .. }
            | Error::HerdrServerTimeout { .. }
            | Error::HerdrServerExited { .. }
            | Error::HerdrStopTimeout { .. }
            | Error::Ack { .. }
            | Error::Json(_)
            | Error::RpcTimeout { .. }
            | Error::RpcClosed { .. }
            | Error::AckId { .. }
            | Error::SubscriptionResult
            | Error::Probe(_)
            | Error::Worker(_)
            | Error::WorkerClosed(_)
            | Error::LifecycleClosed(_)
            | Error::PtyFailure(_)
            | Error::Failed { .. }
            | Error::PtyLimit { .. }
            | Error::InvalidSessionKey(_)
            | Error::InvalidRosterKey(_)
            | Error::Io(_)
            | Error::NamespaceEscape { .. }
            | Error::OwnerUnset
            | Error::SessionStamp(_)
            | Error::Roster(_)) => Self::Unavailable(Box::new(other)),
        }
    }
}
