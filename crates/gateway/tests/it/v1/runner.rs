use std::sync::Arc;

use async_trait::async_trait;
use kernel::agent::{StopReason, Turn, TurnError, TurnResult, TurnRunner};
use kernel::model::{ModelChunk, ModelProvider, ModelProviderError, ModelRequest};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wire::hooks::HookUsage;
use wire::message::{Message, MessageContent, Role};

pub(super) struct FakeRunner {
    provider: Arc<dyn ModelProvider>,
    pub(super) turns: Mutex<Vec<Turn>>,
}

#[derive(Debug, thiserror::Error)]
enum Failure {
    #[error("script failed: {0}")]
    Provider(#[from] ModelProviderError),
    #[error("script missing Done")]
    MissingDone,
    #[error("unexpected tool chunk")]
    Tool,
    #[error("event receiver closed: {0}")]
    Closed(#[from] mpsc::error::SendError<ModelChunk>),
}

impl FakeRunner {
    pub(super) fn new(provider: Arc<dyn ModelProvider>) -> Self {
        Self {
            provider,
            turns: Mutex::new(Vec::new()),
        }
    }

    async fn replay(
        &self,
        request: ModelRequest,
        events: mpsc::Sender<ModelChunk>,
    ) -> Result<TurnResult, Failure> {
        let mut messages = request.messages.clone();
        let mut stream = self.provider.stream(request).await?;
        let mut response = String::new();
        let mut reasoning = String::new();
        let mut usage = None;
        loop {
            let chunk = stream.recv().await.ok_or(Failure::MissingDone)??;
            match &chunk {
                ModelChunk::Text { delta } => response.push_str(delta),
                ModelChunk::Reasoning { delta } => reasoning.push_str(delta),
                ModelChunk::Usage { usage: value } => {
                    usage = Some(HookUsage {
                        prompt_tokens: value.prompt_tokens,
                        completion_tokens: value.completion_tokens,
                    });
                }
                ModelChunk::Done => {}
                ModelChunk::ToolCallStart { .. } | ModelChunk::ToolCallDelta { .. } => {
                    return Err(Failure::Tool);
                }
            }
            let done = chunk == ModelChunk::Done;
            events.send(chunk).await?;
            if done {
                break;
            }
        }
        messages.push(Message {
            role: Role::Assistant,
            content: MessageContent::Text(response.clone()),
            reasoning: if reasoning.is_empty() {
                None
            } else {
                Some(reasoning)
            },
            tool_calls: None,
            tool_call_id: None,
        });
        Ok(TurnResult {
            messages,
            response,
            tools_used: Vec::new(),
            iterations: 1,
            usage,
            stop: StopReason::Completed,
        })
    }
}

#[async_trait]
impl TurnRunner for FakeRunner {
    async fn run(
        &self,
        turn: Turn,
        events: mpsc::Sender<ModelChunk>,
        cancel: CancellationToken,
    ) -> Result<TurnResult, TurnError> {
        let request = turn.request.clone();
        self.turns.lock().push(turn);
        tokio::select! {
            () = cancel.cancelled() => Err(TurnError::Cancelled),
            result = self.replay(request, events) => result.map_err(|error| TurnError::Failed(Box::new(error))),
        }
    }
}
