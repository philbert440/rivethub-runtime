use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Service(#[from] crate::service::ServiceError),
    #[error(transparent)]
    Ca(#[from] kernel::ca::CaError),
    #[error(transparent)]
    Pki(#[from] pki::Error),
    #[error("PKI: {0}")]
    PkiInput(&'static str),
    #[error("holder cryptography failed: {0}")]
    Crypto(#[from] rcgen::Error),
    #[error(transparent)]
    StepCa(#[from] stepca::Error),
    #[error("No config found. Run: rh config init")]
    NoConfig,
    #[error("No config found. Run `rh config init` or use --config <path>")]
    StartConfig,
    #[error(transparent)]
    State(#[from] config::StateError),
    #[error("Failed to parse config: {0}")]
    Config(#[from] config::ConfigError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("blocking command task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("invalid PID file: expected a single PID greater than 1")]
    Pid,
    #[error("process signal failed: {0}")]
    Signal(#[from] nix::errno::Errno),
    #[error("gateway health request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid gateway URL: {0}")]
    Url(#[from] url::ParseError),
    #[error("gateway host or port is invalid")]
    Address,
    #[error("schema verifier failed: rivethub-migrate: {0}")]
    Verifier(#[from] migrate::Error),
    #[error(
        "schema verifier failed: rivethub-migrate: no postgres url — pass --url <url> or --config <file>"
    )]
    PostgresUrl,
    #[error(
        "schema verifier failed: rivethub-migrate: no sqlite path — pass --path <file> or --config <file>"
    )]
    SqlitePath,
    #[error("JSON output failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("command cancelled")]
    Cancelled,
}
