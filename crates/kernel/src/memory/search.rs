use serde::{Deserialize, Serialize};
use wire::gateway::memory::{MemoryDegraded, MemoryFallback};
use wire::memory::{MemoryEntry, MemorySearchResult};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchOutcome {
    pub results: Vec<MemorySearchResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub degraded: Option<MemoryDegraded>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fallback: Option<MemoryFallback>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded: Vec<MemoryEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synthesis: Option<String>,
}
