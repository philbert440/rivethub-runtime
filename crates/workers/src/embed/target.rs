use kernel::memory::EmbedTable;
use kernel::memory::EmbedTarget;
use serde::{Deserialize, Serialize};
use wire::JsonObject;

use crate::error::WorkerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedTarget {
    Ready(EmbedTarget),
    UnknownTable,
    EmptyId,
}

#[derive(Debug, Serialize, Deserialize)]
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

pub(crate) fn embed_payload(target: &EmbedTarget) -> Result<JsonObject, WorkerError> {
    JsonObject::from_serialize(&RawTarget {
        target_table: target.table.to_string(),
        target_id: target.id.clone(),
    })
    .map_err(WorkerError::payload)
}
