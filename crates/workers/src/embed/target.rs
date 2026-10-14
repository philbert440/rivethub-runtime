use async_trait::async_trait;
use serde::Deserialize;
use strum::EnumString;
use wire::JsonObject;

use super::classify::Unembeddable;
use crate::error::WorkerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, strum::Display, EnumString)]
pub enum EmbedTable {
    #[strum(serialize = "ros_messages")]
    Messages,
    #[strum(serialize = "ros_summaries")]
    Summaries,
    #[strum(serialize = "ros_wiki_topics")]
    WikiTopics,
    #[strum(serialize = "ros_message_chunks")]
    MessageChunks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedTarget {
    pub table: EmbedTable,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedTarget {
    pub content: String,
    pub tool_result: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredChunk {
    pub index: u32,
    pub char_start: u32,
    pub char_end: u32,
    pub content: String,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredEmbedding {
    pub target: EmbedTarget,
    pub vector: Vec<f32>,
    pub content_hash: String,
    pub chunks: Vec<StoredChunk>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedTarget {
    Ready(EmbedTarget),
    UnknownTable,
    EmptyId,
}

#[derive(Debug, Deserialize)]
struct RawTarget {
    #[serde(rename = "targetTable")]
    target_table: String,
    #[serde(rename = "targetId")]
    target_id: String,
}

#[must_use]
pub fn embed_key(target: &EmbedTarget) -> String {
    format!("embed-{}-{}", target.table, target.id)
}

pub fn read_target(payload: &JsonObject) -> Result<ParsedTarget, WorkerError> {
    let raw: RawTarget =
        serde_json::from_value(serde_json::to_value(payload).map_err(WorkerError::payload)?)
            .map_err(WorkerError::payload)?;
    let Some(table) = known_table(&raw.target_table) else {
        return Ok(ParsedTarget::UnknownTable);
    };
    if raw.target_id.is_empty() {
        return Ok(ParsedTarget::EmptyId);
    }
    Ok(ParsedTarget::Ready(EmbedTarget {
        table,
        id: raw.target_id,
    }))
}

fn known_table(name: &str) -> Option<EmbedTable> {
    match name {
        "ros_messages" => Some(EmbedTable::Messages),
        "ros_summaries" => Some(EmbedTable::Summaries),
        "ros_wiki_topics" => Some(EmbedTable::WikiTopics),
        "ros_message_chunks" => Some(EmbedTable::MessageChunks),
        _ => None,
    }
}

#[async_trait]
pub trait EmbedTargets: Send + Sync {
    async fn load(&self, target: &EmbedTarget) -> Result<Option<LoadedTarget>, WorkerError>;

    async fn write(&self, stored: StoredEmbedding) -> Result<(), WorkerError>;

    async fn mark_unembeddable(
        &self,
        target: &EmbedTarget,
        reason: Unembeddable,
    ) -> Result<(), WorkerError>;

    async fn record_null(&self, target: &EmbedTarget, error: &str) -> Result<u32, WorkerError>;

    async fn mark_permanent(&self, target: &EmbedTarget, error: &str) -> Result<(), WorkerError>;
}
