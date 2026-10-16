use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use wire::JsonObject;

use super::{EmbedTable, EmbedTarget};
use crate::BoxError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SynthesizeToolCall {
    pub message_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallMessage {
    pub id: Uuid,
    pub tool_name: Option<String>,
    pub tool_args: JsonObject,
    pub content: String,
}

#[derive(Debug, Error)]
pub enum MemoryJobError {
    #[error("memory job store failed: {0}")]
    Query(#[source] BoxError),
    #[error("invalid tool arguments: {0}")]
    Arguments(#[source] BoxError),
    #[error("optional embedding table {table} is missing: {source}")]
    MissingTable {
        table: EmbedTable,
        #[source]
        source: BoxError,
    },
}

#[async_trait]
pub trait MemoryJobStore: Send + Sync {
    async fn tool_call(&self, id: Uuid) -> Result<Option<ToolCallMessage>, MemoryJobError>;

    async fn write_tool_sentence(
        &self,
        id: Uuid,
        previous_content: &str,
        content: &str,
    ) -> Result<(), MemoryJobError>;

    async fn unchunked(
        &self,
        limit: u32,
        chunk_chars: u32,
        tool_result_cap: u32,
    ) -> Result<Vec<EmbedTarget>, MemoryJobError>;

    async fn stale_wiki(
        &self,
        limit: u32,
    ) -> Result<Vec<crate::wiki::builder::ExtractWiki>, MemoryJobError>;

    async fn unembedded(
        &self,
        table: EmbedTable,
        limit: u32,
    ) -> Result<Vec<EmbedTarget>, MemoryJobError>;
}
