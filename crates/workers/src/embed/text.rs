use sha2::Digest;
use sha2::Sha256;

use crate::embed::EmbedTable;
use crate::embed::LoadedTarget;

pub fn compose_message(content: &str, tool_result: Option<&str>, cap: usize) -> String {
    let content = content.trim();
    let tool = capped_tool(tool_result, cap);
    match (content.is_empty(), tool) {
        (true, None) => String::new(),
        (false, None) => content.to_owned(),
        (true, Some(tool)) => tool,
        (false, Some(tool)) => format!("{content}\n{tool}"),
    }
}

pub(super) fn compose(table: EmbedTable, loaded: &LoadedTarget, cap: usize) -> String {
    match table {
        EmbedTable::Messages => {
            compose_message(&loaded.content, loaded.tool_result.as_deref(), cap)
        }
        EmbedTable::Summaries | EmbedTable::WikiTopics | EmbedTable::MessageChunks => {
            loaded.content.trim().to_owned()
        }
    }
}

pub fn content_hash(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

fn capped_tool(tool_result: Option<&str>, cap: usize) -> Option<String> {
    let trimmed = tool_result?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(cap).collect())
}
