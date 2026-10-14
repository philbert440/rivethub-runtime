use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::Utc;
use kernel::ToolContext;
use kernel::agent::{StopReason, Turn, TurnLimits, TurnResult, TurnRunner};
use kernel::model::{ModelChunk, ModelRequest};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::DropGuard;
use uuid::Uuid;
use wire::JsonObject;
use wire::gateway::v1::{ChatRequest, Completion, CompletionChoice};
use wire::hooks::HookContextBase;
use wire::message::{Message, MessageContent, Role};
use wire::stream::openai::{self, FinishReason, Usage};

use super::storage::Lease;
use super::{Error, Service, conversations, service, streaming};

pub(super) struct Running {
    pub(super) events: mpsc::Receiver<ModelChunk>,
    pub(super) result: oneshot::Receiver<Result<Completion, Error>>,
    pub(super) guard: DropGuard,
    pub(super) id: String,
    pub(super) created: i64,
    pub(super) model: String,
}

pub(super) async fn complete(
    State(gateway): State<Arc<crate::Gateway>>,
    extension: Option<Extension<Arc<Service>>>,
    headers: HeaderMap,
    body: Result<Json<ChatRequest>, JsonRejection>,
) -> Result<Response, Error> {
    let service = service(extension)?;
    let runner = Arc::clone(gateway.turn_runner.as_ref().ok_or(Error::Unavailable)?);
    let request = super::body(body)?;
    let messages = validate(&request)?;
    let streaming = request.stream == Some(true);
    let include_usage = request
        .stream_options
        .as_ref()
        .is_some_and(|options| options.include_usage);
    let (conversation, lease) = conversation(&service, &headers, &messages).await?;
    let header =
        HeaderValue::from_str(&conversation).map_err(|error| Error::Invalid(error.to_string()))?;
    let running = start(&service, runner, request, messages, conversation, lease);
    let mut response = if streaming {
        streaming::response(running, include_usage)
    } else {
        let Running {
            mut events,
            result,
            guard,
            ..
        } = running;
        let drain = async { while events.recv().await.is_some() {} };
        let (result, ()) = tokio::join!(result, drain);
        drop(guard);
        Json(result.map_err(|_| Error::Unavailable)??).into_response()
    };
    response
        .headers_mut()
        .insert("x-rivet-conversation", header);
    Ok(response)
}

fn validate(request: &ChatRequest) -> Result<Vec<Message>, Error> {
    if request.model.trim().is_empty() {
        return Err(Error::Invalid("model must not be empty".to_owned()));
    }
    if request.max_tokens == Some(0) {
        return Err(Error::Invalid("max_tokens must be positive".to_owned()));
    }
    if request.messages.last().map(|message| message.role) != Some(Role::User) {
        return Err(Error::Invalid(
            "messages must end with a user message".to_owned(),
        ));
    }
    request
        .messages
        .iter()
        .map(|message| {
            if message.tool_calls.is_some()
                || message.tool_call_id.is_some()
                || message.role == Role::Tool
            {
                return Err(Error::Invalid(
                    "headless turns do not accept tool messages".to_owned(),
                ));
            }
            let content = message
                .content
                .as_ref()
                .ok_or_else(|| Error::Invalid("message content is required".to_owned()))?;
            Ok(Message {
                role: message.role,
                content: MessageContent::Text(content.clone()),
                reasoning: message.reasoning_content.clone(),
                tool_calls: None,
                tool_call_id: None,
            })
        })
        .collect()
}

async fn conversation(
    service: &Service,
    headers: &HeaderMap,
    messages: &[Message],
) -> Result<(String, Lease), Error> {
    let id = if let Some(id) = headers.get("x-rivet-conversation") {
        conversations::identifier(
            id.to_str()
                .map_err(|error| Error::Invalid(error.to_string()))?,
        )?
    } else {
        let title = if let Some(title) = headers.get("x-rivet-title") {
            conversations::title(
                title
                    .to_str()
                    .map_err(|error| Error::Invalid(error.to_string()))?
                    .to_owned(),
            )?
        } else {
            let Some(Message {
                content: MessageContent::Text(text),
                ..
            }) = messages.last()
            else {
                return Err(Error::Invalid("user text is required".to_owned()));
            };
            conversations::title(text.chars().take(80).collect())?
        };
        let record = service
            .conversations
            .create(
                title,
                Utc::now().timestamp_millis(),
                &service.tasks,
                &service.cancel,
            )
            .await?;
        conversations::identifier(&record.id)?
    };
    let entry = service.conversations.entry(id)?;
    let lease = entry.lease()?;
    entry.snapshot()?;
    Ok((id.to_string(), lease))
}

fn start(
    service: &Service,
    runner: Arc<dyn TurnRunner>,
    request: ChatRequest,
    messages: Vec<Message>,
    conversation: String,
    lease: Lease,
) -> Running {
    let cancel = service.cancel.child_token();
    let guard = cancel.clone().drop_guard();
    let (sender, events) = mpsc::channel(32);
    let (reply, result) = oneshot::channel();
    let id = format!("chatcmpl-{}", Uuid::new_v4());
    let created = Utc::now().timestamp();
    let model = request.model.clone();
    let completion_id = id.clone();
    let tasks = service.tasks.clone();
    let turn = turn(request, messages, conversation, service.limits);
    let result_model = model.clone();
    tasks.spawn(async move {
        let outcome = runner.run(turn, sender, cancel).await;
        let outcome = match outcome {
            Ok(result) => finish((result, result_model), completion_id, created, lease).await,
            Err(error) => Err(error.into()),
        };
        if let Err(outcome) = reply.send(outcome)
            && let Err(error) = outcome
        {
            tracing::debug!(%error, "headless request disconnected");
        }
    });
    Running {
        events,
        result,
        guard,
        id,
        created,
        model,
    }
}

fn turn(
    request: ChatRequest,
    messages: Vec<Message>,
    conversation: String,
    limits: TurnLimits,
) -> Turn {
    Turn {
        request: ModelRequest {
            model: request.model,
            messages,
            tools: Vec::new(),
            thinking: None,
            max_tokens: request.max_tokens,
        },
        context: HookContextBase {
            agent_id: None,
            session_id: Some(conversation),
            timestamp: Utc::now(),
            metadata: JsonObject::default(),
        },
        tool_context: ToolContext {
            agent_id: None,
            working_dir: None,
            session: None,
        },
        authorized_tools: BTreeSet::new(),
        limits,
    }
}

async fn finish(
    (result, model): (TurnResult, String),
    id: String,
    created: i64,
    lease: Lease,
) -> Result<Completion, Error> {
    let finish_reason = match result.stop {
        StopReason::Completed => FinishReason::Stop,
        StopReason::StepLimit => FinishReason::Length,
    };
    let usage = result
        .usage
        .map(|usage| {
            Ok::<_, Error>(Usage {
                total_tokens: usage
                    .prompt_tokens
                    .checked_add(usage.completion_tokens)
                    .ok_or(Error::UsageOverflow)?,
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                prompt_tokens_details: None,
                completion_tokens_details: None,
            })
        })
        .transpose()?;
    let messages = result
        .messages
        .into_iter()
        .map(|message| {
            let MessageContent::Text(content) = message.content else {
                return Err(Error::Invalid("expected text content".to_owned()));
            };
            Ok(openai::Message {
                role: message.role,
                content: Some(content),
                reasoning_content: message.reasoning,
                tool_calls: None,
                tool_call_id: None,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let message = messages.last().cloned().ok_or(Error::Unavailable)?;
    lease.save(messages, Utc::now().timestamp_millis()).await?;
    Ok(Completion {
        id,
        created,
        model,
        choices: vec![CompletionChoice {
            index: 0,
            message,
            finish_reason,
        }],
        usage,
    })
}
