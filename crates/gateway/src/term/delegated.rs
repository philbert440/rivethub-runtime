use std::collections::BTreeMap;

use wire::harness::HarnessId;

use super::TerminalService;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegate {
    pub harness: HarnessId,
    pub model: Option<String>,
    pub cwd: Option<std::path::PathBuf>,
    pub permission_mode: Option<String>,
}

impl TerminalService {
    #[must_use]
    pub fn with_delegates(mut self, delegates: BTreeMap<String, Delegate>) -> Self {
        self.delegates = delegates;
        self
    }

    pub(crate) fn delegate(&self, agent: &str) -> Option<&Delegate> {
        self.delegates.get(agent)
    }
}
