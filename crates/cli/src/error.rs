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
    #[error(transparent)]
    Pki(#[from] pki::Error),
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
    #[error("{0}")]
    Migrate(migrate::Error),
    #[error("rh local runs on SQLite; this config selects Postgres")]
    PostgresBackend,
    #[error("rh local needs memory.sqlite.path set to a database file")]
    LocalDatabase,
    #[error("rh local --port must be between 1 and 65535")]
    GatewayPort,
    #[error("refusing to replace the config at {}", path.display())]
    ConfigPresent { path: std::path::PathBuf },
    #[error("the node claim was already released")]
    ClaimMissing,
    #[error("JSON output failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("command cancelled")]
    Cancelled,
    #[error("--name <node> is required")]
    NameRequired,
    #[error(
        "invalid node name \"{name}\" — must match [a-z0-9]([a-z0-9-]*[a-z0-9])? and be at most 63 characters"
    )]
    NodeName { name: String },
    #[error("expected user@host, got \"{spec}\"")]
    Target { spec: String },
    #[error("refusing unsafe user@host")]
    UnsafeTarget,
    #[error("refusing unsafe --hub-cmd")]
    UnsafeHub,
    #[error("refusing unsafe --advertise")]
    UnsafeAdvertise,
    #[error("refusing unsafe host")]
    UnsafeHost,
    #[error("--advertise <host|ip> is required")]
    AdvertiseRequired,
    #[error("no mesh file")]
    NoMesh,
    #[error("no mesh nodes")]
    NoNodes,
    #[error("mesh node \"{id}\" is not in the registry")]
    UnknownMeshNode { id: String },
    #[error("refusing to forget local node \"{name}\" without --force")]
    ForgetLocal { name: String },
    #[error("port must be between 1 and 65535")]
    MeshPort,
    #[error("ping timeout must be a positive number of milliseconds")]
    MeshTimeout,
    #[error(
        "step-ca enrollment needs --ca-url, --root, and --provisioner-password-file. Mint with rh ca token node <name> against roster {roster}; the holder key stays in the state directory and POST /1.0/sign issues the certificate. rivet-ca.sh is retired"
    )]
    StepCaEnroll { roster: String },
    #[error(
        "step-ca renewal needs --ca-url and --root. rh cert renew uses mTLS against step-ca. rivet-ca.sh is retired"
    )]
    StepCaRenew,
    #[error("join response was not a mesh registry")]
    JoinBody,
    #[error("seed node did not return a mesh registry")]
    SeedBody,
    #[error("hub mesh.json was not a mesh registry")]
    HubBody,
    #[error("cannot reach hub mesh: {0}")]
    HubReach(String),
    #[error("hub mesh responded with HTTP {code}")]
    HubStatus { code: u16 },
    #[error("refusing to write holder keys under /rivet-shared")]
    SharedKey,
    #[error("mesh file must be an absolute path inside a directory")]
    MeshPath,
    #[error("the operating-system hostname is not Unicode")]
    HostName,
    #[error("mesh HTTP body was not UTF-8")]
    MeshBody,
    #[error("holder state must be an absolute path")]
    StatePath,
    #[error(transparent)]
    Registry(#[from] gateway::mesh::RegistryError),
    #[error("RIVETHUB_PG_URL is required.")]
    PostgresUrlRequired,
    #[error("Unknown option: {0}")]
    UnknownOption(String),
    #[error("--task requires a task identifier (e.g. extract-wiki)")]
    TaskRequired,
    #[error("--error requires a substring to match against last_error")]
    ErrorSubstringRequired,
    #[error(
        "At least one --task <identifier> is required (e.g. --task extract-wiki). Refusing to reset every dead job without an explicit task filter."
    )]
    RetryTaskFilter,
    #[error(
        "At least one --task <identifier> is required (e.g. --task extract-wiki). Refusing to requeue every dead job without an explicit task filter."
    )]
    RequeueTaskFilter,
    #[error("--task {task} is not an allowed memory task ({allowed})")]
    TaskNotAllowed { task: String, allowed: &'static str },
    #[error("requeue accepts one --task")]
    OneTask,
    #[error("--limit requires a positive integer")]
    LimitPositive,
    #[error("--limit requires an integer")]
    LimitInteger,
    #[error("--out requires a file path")]
    OutRequired,
    #[error("--since requires an ISO timestamp")]
    SinceRequired,
    #[error("invalid --since timestamp: {0}")]
    SinceInvalid(String),
    #[error("refusing to write gzip to a TTY (redirect stdout or pass --out <file>)")]
    GzipTty,
    #[error("import requires a file path")]
    ImportPath,
    #[error("import accepts one file path")]
    ImportOne,
    #[error("--sqlite requires a file path")]
    SqliteRequired,
    #[error("unexpected argument: {0}")]
    Unexpected(String),
    #[error("output path is a symlink")]
    OutputSymlink,
    #[error("output directory does not exist")]
    OutputDirectory,
    #[error("output path has no file name")]
    OutputName,
    #[error("hostname is unavailable")]
    HostUnavailable,
    #[error("invalid RIVETHUB_PG_URL: {0}")]
    MemoryUrl(url::ParseError),
    #[error("RIVETHUB_PG_URL has no host")]
    MemoryHost,
    #[error("rh logs accepts one agent identifier")]
    OneAgent,
    #[error("--lines requires an integer")]
    LinesInteger,
    #[error("--level requires a syslog level")]
    LevelRequired,
    #[error("unknown syslog level: {0}")]
    LevelUnknown(String),
    #[error("--since requires a time")]
    JournalSince,
    #[error("--grep requires text")]
    GrepRequired,
    #[error("unknown export source: {0}")]
    UnknownDumpSource(String),
    #[error("memory dump is not UTF-8")]
    DumpEncoding,
    #[error("export host is empty")]
    DumpHost,
    #[error(transparent)]
    MemoryPostgres(#[from] memory_postgres::Error),
    #[error(transparent)]
    MemorySqlite(#[from] memory_sqlite::SqliteError),
    #[error(transparent)]
    MemoryImport(#[from] memory_sqlite::import::ImportError),
}
