use async_trait::async_trait;
use thiserror::Error;
use wire::gateway::agents::AgentPreset;
use wire::gateway::agents::AgentPresetUpdate;
use wire::gateway::agents::AgentStoreBackend;

use crate::BoxError;

#[derive(Debug, Error)]
pub enum AgentPresetStoreError {
    #[error("an agent named \"{name}\" already exists")]
    NameTaken { name: String },
    #[error("agent preset store is unavailable")]
    Unavailable(#[source] BoxError),
}

#[async_trait]
pub trait AgentPresetStore: Send + Sync {
    fn backend(&self) -> AgentStoreBackend;

    async fn list(&self, node: Option<&str>) -> Result<Vec<AgentPreset>, AgentPresetStoreError>;

    async fn get(&self, id: &str) -> Result<Option<AgentPreset>, AgentPresetStoreError>;

    async fn lookup(&self, id_or_name: &str) -> Result<Option<AgentPreset>, AgentPresetStoreError>;

    async fn create(&self, preset: AgentPreset) -> Result<AgentPreset, AgentPresetStoreError>;

    async fn update(
        &self,
        id: &str,
        changes: AgentPresetUpdate,
    ) -> Result<Option<AgentPreset>, AgentPresetStoreError>;

    async fn delete(&self, id: &str) -> Result<bool, AgentPresetStoreError>;
}
