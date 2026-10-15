use std::fmt;

use config::ServiceSwitch;
use kernel::queue::WorkerRole;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceRoot {
    Gateway,
    Agent,
    Worker(WorkerRole),
    Mcp,
}

impl fmt::Display for ServiceRoot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gateway => write!(formatter, "gateway"),
            Self::Agent => write!(formatter, "agent"),
            Self::Worker(role) => write!(formatter, "worker {role}"),
            Self::Mcp => write!(formatter, "mcp"),
        }
    }
}

#[must_use]
pub(crate) fn roots_from(switches: &[ServiceSwitch]) -> Vec<ServiceRoot> {
    let mut roots = Vec::new();
    for switch in switches {
        match switch {
            ServiceSwitch::Gateway => roots.push(ServiceRoot::Gateway),
            ServiceSwitch::Agent => roots.push(ServiceRoot::Agent),
            ServiceSwitch::Workers => {
                roots.push(ServiceRoot::Worker(WorkerRole::Embedder));
                roots.push(ServiceRoot::Worker(WorkerRole::Summarizer));
            }
            ServiceSwitch::Tagging => roots.push(ServiceRoot::Worker(WorkerRole::Categorizer)),
            ServiceSwitch::Wiki => roots.push(ServiceRoot::Worker(WorkerRole::WikiBuilder)),
            ServiceSwitch::Mcp => roots.push(ServiceRoot::Mcp),
        }
    }
    roots
}
