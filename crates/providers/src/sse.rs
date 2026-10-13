use eventsource_stream::EventStreamError;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use kernel::model::ModelChunk;
use kernel::model::ModelProviderError;
use kernel::model::ModelStreamItem;
use wire::stream::openai::ChatCompletionChunk;
use wire::stream::openai::StreamedToolCall;

pub const STREAM_CAPACITY: usize = 64;

pub async fn pump(response: reqwest::Response, tx: tokio::sync::mpsc::Sender<ModelStreamItem>) {
    let mut events = response.bytes_stream().eventsource();
    let mut complete = false;
    loop {
        tokio::select! {
            biased;
            () = tx.closed() => return,
            next = events.next() => {
                match next {
                    None => break,
                    Some(Ok(event)) => {
                        if event.data.trim() == "[DONE]" {
                            complete = true;
                            break;
                        }
                        if !forward(&tx, &event.data).await {
                            return;
                        }
                    }
                    Some(Err(source)) => {
                        send_failure(&tx, stream_event_error(source)).await;
                        return;
                    }
                }
            }
        }
    }
    if complete {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the receiver is already gone"
        )]
        #[expect(
            clippy::let_underscore_untyped,
            reason = "the receiver is already gone"
        )]
        let _ = tx.send(Ok(ModelChunk::Done)).await;
    }
}

async fn forward(tx: &tokio::sync::mpsc::Sender<ModelStreamItem>, data: &str) -> bool {
    let chunk: ChatCompletionChunk = match serde_json::from_str(data) {
        Ok(chunk) => chunk,
        Err(source) => {
            send_failure(
                tx,
                stream_error(
                    format!("the model server sent a malformed stream event: {source}"),
                    source,
                ),
            )
            .await;
            return false;
        }
    };
    let mut translated = Vec::new();
    if let Err(message) = translate(&chunk, &mut translated) {
        send_failure(tx, stream_error(message.clone(), message)).await;
        return false;
    }
    for chunk in translated {
        if tx.send(Ok(chunk)).await.is_err() {
            return false;
        }
    }
    true
}

async fn send_failure(tx: &tokio::sync::mpsc::Sender<ModelStreamItem>, error: ModelProviderError) {
    #[expect(
        clippy::let_underscore_must_use,
        reason = "the receiver is already gone"
    )]
    #[expect(
        clippy::let_underscore_untyped,
        reason = "the receiver is already gone"
    )]
    let _ = tx.send(Err(error)).await;
}

fn stream_event_error(source: EventStreamError<reqwest::Error>) -> ModelProviderError {
    let malformed = matches!(
        &source,
        EventStreamError::Parser(_) | EventStreamError::Utf8(_)
    );
    let message = if malformed {
        format!("the model server sent a malformed stream event: {source}")
    } else {
        format!("the model server stream failed: {source}")
    };
    stream_error(message, source)
}

fn translate(chunk: &ChatCompletionChunk, out: &mut Vec<ModelChunk>) -> Result<(), String> {
    for choice in &chunk.choices {
        if let Some(text) = non_empty(choice.delta.content.as_deref()) {
            out.push(ModelChunk::Text {
                delta: text.to_owned(),
            });
        }
        if let Some(reasoning) = non_empty(choice.delta.reasoning_content.as_deref()) {
            out.push(ModelChunk::Reasoning {
                delta: reasoning.to_owned(),
            });
        }
        for call in choice.delta.tool_calls.iter().flatten() {
            let call = match call {
                StreamedToolCall::Function(tagged) => match tagged {
                    wire::stream::openai::FunctionToolCallTag::Function(plain) => plain,
                },
                StreamedToolCall::Plain(plain) => plain,
            };
            let index = call.index.ok_or_else(|| {
                "the model server sent a tool call chunk without an index".to_owned()
            })?;
            if let Some(name) = non_empty(call.function.name.as_deref()) {
                let id = call
                    .id
                    .as_deref()
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        "the model server started a tool call without an id".to_owned()
                    })?;
                out.push(ModelChunk::ToolCallStart {
                    index,
                    id: id.to_owned(),
                    name: name.to_owned(),
                });
            }
            if let Some(arguments) = non_empty(call.function.arguments.as_deref()) {
                out.push(ModelChunk::ToolCallDelta {
                    index,
                    arguments_delta: arguments.to_owned(),
                });
            }
        }
    }
    if let Some(usage) = &chunk.usage {
        out.push(ModelChunk::Usage {
            usage: wire::stream::LlmUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                reasoning_tokens: usage
                    .completion_tokens_details
                    .as_ref()
                    .map(|d| d.reasoning_tokens),
                cached_tokens: usage
                    .prompt_tokens_details
                    .as_ref()
                    .map(|d| d.cached_tokens),
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        });
    }
    Ok(())
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    Some(value)
}

pub(crate) fn stream_error(
    message: impl Into<String>,
    source: impl Into<kernel::BoxError>,
) -> ModelProviderError {
    ModelProviderError::Stream {
        message: message.into(),
        source: source.into(),
    }
}
