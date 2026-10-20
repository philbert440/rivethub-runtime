use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("cannot read this node's hostname: {0}")]
    Hostname(#[from] nix::errno::Errno),
    #[error("this node's hostname is not Unicode")]
    HostnameEncoding,
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error("update requires Linux x86_64 or aarch64")]
    Platform,
    #[error("invalid release version: {0}")]
    Version(String),
    #[error("no matching published release contains both binaries for this platform")]
    Release,
    #[error("release download failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("release asset is too large")]
    AssetSize,
    #[error("release checksum manifest is invalid")]
    Manifest,
    #[error("SHA-256 verification failed for {0}")]
    Checksum(String),
    #[error("update path is outside its root or is not a regular file: {}", .0.display())]
    Path(PathBuf),
    #[error("another update is in progress")]
    Busy,
    #[error("source updates require RIVETHUB_INSTALL_ROOT pointing to a checkout")]
    Source,
    #[error("source checkout must be clean and on branch {0}")]
    Branch(&'static str),
    #[error("source checkout does not match the requested version")]
    Revision,
    #[error("mesh registry was not found")]
    MeshMissing,
    #[error("invalid SSH host or user for node {0}")]
    SshDestination(String),
    #[error("remote argument contains a NUL byte")]
    ShellArgument,
    #[error("mesh registry: {0}")]
    Registry(#[from] gateway::mesh::RegistryError),
    #[error("update I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("update task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("update command failed: {0}")]
    Command(#[from] crate::service::ServiceError),
    #[error("update cancelled")]
    Cancelled,
}
