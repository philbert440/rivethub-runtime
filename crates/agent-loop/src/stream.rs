use std::collections::{BTreeMap, BTreeSet};

use kernel::model::{ModelChunk, ModelStreamItem};
use tokio::sync::mpsc;
use wire::message::{Message, MessageContent, Role, ToolCall};
use wire::stream::LlmUsage;

use crate::Error;

#[derive(Debug)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
}

#[derive(Debug)]
pub struct Response {
    pub message: Message,
    pub usage: Option<LlmUsage>,
}

pub async fn collect(
    mut receiver: mpsc::Receiver<ModelStreamItem>,
    events: &mpsc::Sender<ModelChunk>,
    byte_limit: usize,
) -> Result<Response, Error> {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls = BTreeMap::<u32, PendingCall>::new();
    let mut ids = BTreeSet::new();
    let mut usage = None;
    let mut bytes = 0_usize;
    loop {
        let chunk = receiver
            .recv()
            .await
            .ok_or_else(|| Error::Stream("missing completion marker".to_owned()))??;
        bytes = bytes
            .checked_add(chunk_bytes(&chunk))
            .ok_or_else(|| Error::Stream("response size overflow".to_owned()))?;
        if bytes > byte_limit {
            return Err(Error::Stream("response exceeds byte limit".to_owned()));
        }
        match &chunk {
            ModelChunk::Text { delta } => text.push_str(delta),
            ModelChunk::Reasoning { delta } => reasoning.push_str(delta),
            ModelChunk::ToolCallStart { index, id, name } => {
                if calls.contains_key(index)
                    || !ids.insert(id.clone())
                    || id.is_empty()
                    || name.is_empty()
                {
                    return Err(Error::Stream("invalid or duplicate tool call".to_owned()));
                }
                calls.insert(
                    *index,
                    PendingCall {
                        id: id.clone(),
                        name: name.clone(),
                        arguments: String::new(),
                    },
                );
            }
            ModelChunk::ToolCallDelta {
                index,
                arguments_delta,
            } => {
                calls
                    .get_mut(index)
                    .ok_or_else(|| Error::Stream(format!("unknown tool call index {index}")))?
                    .arguments
                    .push_str(arguments_delta);
            }
            ModelChunk::Usage { usage: measured } => usage = Some(measured.clone()),
            ModelChunk::Done => {
                events.send(chunk).await.map_err(|_| Error::Disconnected)?;
                break;
            }
        }
        events.send(chunk).await.map_err(|_| Error::Disconnected)?;
    }
    finish(text, reasoning, calls, usage)
}

fn finish(
    text: String,
    reasoning: String,
    calls: BTreeMap<u32, PendingCall>,
    usage: Option<LlmUsage>,
) -> Result<Response, Error> {
    let tool_calls = calls
        .into_values()
        .map(|call| {
            Ok(ToolCall {
                id: call.id,
                name: call.name,
                arguments: serde_json::from_str(&call.arguments)?,
                thought_signature: None,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(Response {
        message: Message {
            role: Role::Assistant,
            content: MessageContent::Text(text),
            reasoning: if reasoning.is_empty() {
                None
            } else {
                Some(reasoning)
            },
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
        },
        usage,
    })
}

fn chunk_bytes(chunk: &ModelChunk) -> usize {
    match chunk {
        ModelChunk::Text { delta } | ModelChunk::Reasoning { delta } => delta.len(),
        ModelChunk::ToolCallStart { id, name, .. } => id.len().saturating_add(name.len()),
        ModelChunk::ToolCallDelta {
            arguments_delta, ..
        } => arguments_delta.len(),
        ModelChunk::Usage { .. } | ModelChunk::Done => 0,
    }
}
