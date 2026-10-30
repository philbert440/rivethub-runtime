use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use thiserror::Error;
use values::Extra;
use values::HarnessId;

use crate::BoxError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPreset {
    pub id: String,
    pub name: String,
    pub color: String,
    pub harness_id: Option<HarnessId>,
    pub model: String,
    pub effort: String,
    pub system_prompt: String,
    pub node: Option<String>,
    pub directory: Option<String>,
    pub shared_link: Option<bool>,
    pub node_base_url: Option<String>,
    pub sort_order: Option<u32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentPresetUpdate {
    pub name: Option<String>,
    pub color: Option<String>,
    pub harness_id: Option<Option<HarnessId>>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub system_prompt: Option<String>,
    pub node: Option<String>,
    pub directory: Option<String>,
    pub shared_link: Option<bool>,
    pub node_base_url: Option<String>,
    pub sort_order: Option<Option<u32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStoreBackend {
    Postgres,
    File,
}

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
