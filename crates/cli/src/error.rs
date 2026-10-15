use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Service(#[from] crate::service::ServiceError),
    #[error(transparent)]
    Ca(#[from] kernel::ca::CaError),
    #[error("PKI: {0}")]
    PkiInput(&'static str),
    #[error("rh ca requires --ca-url")]
    CaUrlRequired,
    #[error("rh ca requires --root")]
    RootRequired,
    #[error("rh ca token and rh ca revoke require --provisioner-password-file")]
    PasswordRequired,
    #[error("rh ca token node and rh ca token agent require --roster")]
    RosterRequired,
    #[error("certificate authority file is not a regular file")]
    CaFile,
    #[error("certificate authority path escapes its directory")]
    CaEscape,
    #[error("certificate authority path contains traversal")]
    CaTraversal,
    #[error("certificate authority file is not UTF-8")]
    CaEncoding,
    #[error("the operator roster cannot be mesh.json, because that registry is self-registered")]
    MeshRegistry,
    #[error("operator roster line {line} is malformed")]
    RosterLine { line: usize },
    #[error("operator roster line {line} repeats node `{name}`")]
    RosterNode { line: usize, name: String },
    #[error("operator roster line {line} repeats agent `{name}`")]
    RosterAgent { line: usize, name: String },
    #[error("operator roster line {line} repeats address {address}")]
    RosterAddress {
        line: usize,
        address: std::net::IpAddr,
    },
    #[error("node `{0}` is not in the operator roster")]
    UnknownNode(String),
    #[error("agent `{0}` is not in the operator roster")]
    UnknownAgent(String),
    #[error("device name is not a single unreserved name")]
    DeviceName,
    #[error("certificate serial is not a canonical decimal integer")]
    Serial,
    #[error("step ca {action} failed: {detail}")]
    StepFailed {
        action: &'static str,
        detail: String,
    },
    #[error("step ca returned an empty or invalid one-time token")]
    StepToken,
    #[error("step ca output is not valid UTF-8")]
    StepEncoding,
    #[error("step ca output exceeds 1 MiB")]
    StepOutput,
    #[error("step ca did not provide pipes")]
    StepPipe,
    #[error("certificate authority root is not an X.509 certificate")]
    CaCertificate,
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
