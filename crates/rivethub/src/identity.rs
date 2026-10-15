use config::Config;
use config::Environment;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("the operating-system hostname is empty")]
    Empty,
    #[error("the operating-system hostname is not Unicode")]
    NotUnicode,
    #[error("the operating-system hostname could not be read")]
    Unreadable {
        #[source]
        source: nix::errno::Errno,
    },
    #[error(transparent)]
    Config(#[from] config::ConfigError),
}

#[derive(Debug)]
pub(crate) struct NodeIdentity {
    pub name: String,
    pub hostname: String,
}

pub(crate) fn resolve(config: &Config, env: &Environment) -> Result<NodeIdentity, NodeError> {
    let hostname = read_host_name()?;
    let name = config::resolve_node_name(config, env, &hostname)?;
    Ok(NodeIdentity { name, hostname })
}

pub(crate) fn announce_resolved(
    root: &str,
    config: &Config,
    env: &Environment,
) -> Result<NodeIdentity, NodeError> {
    let identity = resolve(config, env)?;
    announce(root, &identity);
    Ok(identity)
}

pub(crate) fn announce(root: &str, identity: &NodeIdentity) {
    let name = &identity.name;
    let hostname = &identity.hostname;
    tracing::info!("service node {root} {name} hostname {hostname}");
}

fn read_host_name() -> Result<String, NodeError> {
    let raw = nix::unistd::gethostname()?;
    let text = raw.into_string().map_err(|_| NodeError::NotUnicode)?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        Err(NodeError::Empty)
    } else {
        Ok(trimmed.to_owned())
    }
}

impl From<nix::errno::Errno> for NodeError {
    fn from(source: nix::errno::Errno) -> Self {
        Self::Unreadable { source }
    }
}
