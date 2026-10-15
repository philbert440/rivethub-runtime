use crate::Environment;
use crate::model::{Config, TransportSection};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceSwitch {
    Gateway,
    Agent,
    Workers,
    Tagging,
    Wiki,
    Mcp,
}

impl Config {
    #[must_use]
    pub fn service_switches(&self, env: &Environment) -> Vec<ServiceSwitch> {
        let mut switches = Vec::new();
        if self
            .gateway
            .as_ref()
            .is_none_or(|section| section.enabled != Some(false))
        {
            switches.push(ServiceSwitch::Gateway);
        }
        if agent_root(self, env) {
            switches.push(ServiceSwitch::Agent);
        }
        if let Some(sqlite) = self
            .memory
            .as_ref()
            .and_then(|memory| memory.sqlite.as_ref())
        {
            if sqlite.workers == Some(true) {
                switches.push(ServiceSwitch::Workers);
            }
            if sqlite.tagging == Some(true) {
                switches.push(ServiceSwitch::Tagging);
            }
            if sqlite.wiki_dir.is_some() {
                switches.push(ServiceSwitch::Wiki);
            }
        }
        if self
            .transports
            .as_ref()
            .and_then(|transports| transports.get("mcp"))
            .is_some_and(mcp_enabled)
        {
            switches.push(ServiceSwitch::Mcp);
        }
        switches
    }
}

fn agent_root(config: &Config, env: &Environment) -> bool {
    config.postgres_selected(env)
}

fn mcp_enabled(section: &TransportSection) -> bool {
    section.host.is_some() || section.socket.is_some()
}
