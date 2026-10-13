use kernel::terminal::TerminalId;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("terminal mux is unavailable: {0}")]
    MuxUnavailable(String),
    #[error("herdr not found on PATH or in ~/.local/bin")]
    HerdrNotFound,
    #[error("herdr at {binary} is not {pinned} (den pins {pinned} / protocol 20)")]
    HerdrVersion {
        binary: Box<std::path::Path>,
        pinned: String,
        found: String,
    },
    #[error("failed to init herdr ({message})")]
    HerdrInit { message: String },
    #[error("socket path too long ({len} > 107): {path}")]
    SocketPathTooLong {
        len: usize,
        path: Box<std::path::Path>,
    },
    #[error("herdr server did not open {socket} within {timeout_ms}ms for {session}")]
    HerdrServerTimeout {
        socket: Box<std::path::Path>,
        timeout_ms: u64,
        session: String,
    },
    #[error("herdr server stop failed for {session}: {message}")]
    HerdrStop { session: String, message: String },
    #[error("herdr server did not release {socket} within {timeout_ms}ms for {session}")]
    HerdrStopTimeout {
        socket: Box<std::path::Path>,
        timeout_ms: u64,
        session: String,
    },
    #[error("herdr rpc {method}: {message}")]
    Rpc {
        method: std::borrow::Cow<'static, str>,
        message: String,
    },
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
    #[error("terminal spawn failed: {0}")]
    Spawn(String),
    #[error("terminal io failed: {0}")]
    Io(std::io::Error),
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<Error> for kernel::terminal::TerminalMuxError {
    fn from(value: Error) -> Self {
        match value {
            Error::UnknownTerminal { terminal } => Self::UnknownTerminal(terminal),
            Error::Exited { terminal } => Self::Exited(terminal),
            other @ (Error::MuxUnavailable(_)
            | Error::HerdrNotFound
            | Error::HerdrVersion { .. }
            | Error::HerdrInit { .. }
            | Error::SocketPathTooLong { .. }
            | Error::HerdrServerTimeout { .. }
            | Error::HerdrStop { .. }
            | Error::HerdrStopTimeout { .. }
            | Error::Rpc { .. }
            | Error::PtyLimit { .. }
            | Error::InvalidSessionKey(_)
            | Error::InvalidRosterKey(_)
            | Error::Spawn(_)
            | Error::Io(_)) => Self::Unavailable(Box::new(other)),
        }
    }
}
