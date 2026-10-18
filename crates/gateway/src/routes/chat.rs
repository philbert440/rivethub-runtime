use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::Utc;
use kernel::ToolContext;
use kernel::agent::{Turn, TurnError, TurnLimits, TurnResult, TurnRunner, excluded_names};
use kernel::model::ModelRequest;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::gateway::sessions::{
    SessionMessage, SessionMessageRole, SessionPostAccepted, SessionPostReply, SessionPostRequest,
};
use wire::harness::{HarnessId, HarnessSessionSummary};
use wire::hooks::HookContextBase;
use wire::message::{Message, MessageContent, Role};

use super::sessions::Accepted;
use super::terminal::{require, service};
use crate::Gateway;
use crate::error::ApiError;
use crate::extract::Body;
use crate::tenancy::UserId;

#[derive(Debug, Deserialize)]
struct Create {
    harness: HarnessId,
    model: Option<String>,
    cwd: Option<std::path::PathBuf>,
}

pub(crate) fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route("/api/harness-sessions", post(create).get(list))
        .route("/term/harness-sessions", get(list))
        .route("/api/terminal/harness-sessions", get(list))
        .route(
            "/term/harness-sessions/{session}/transcript",
            get(transcript),
        )
        .route(
            "/api/terminal/harness-sessions/{session}/transcript",
            get(transcript),
        )
        .route("/api/harness-sessions/{session}/turn", post(prompt))
        .route("/api/harness-sessions/{session}/stop", post(stop))
        .route("/api/sessions/{session}/messages", post(prompt))
}

async fn list(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
) -> Result<Json<wire::harness::HarnessSessionList>, ApiError> {
    Ok(Json(require(&gateway)?.harness_sessions(&user)))
}

async fn transcript(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path(session): Path<String>,
) -> Result<Json<wire::gateway::sessions::TranscriptResponse>, ApiError> {
    require(&gateway)?;
    gateway.sessions.authorize(&user, &session)?;
    let reader = gateway
        .transcript
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("transcript reader unavailable".into()))?;
    let canonical = session
        .parse::<wire::session_id::SessionId>()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let command = canonical
        .as_str()
        .split_once(':')
        .ok_or_else(|| ApiError::BadRequest("canonical session required".into()))?
        .0
        .to_owned();
    let turns = reader
        .read(&canonical)
        .await?
        .into_iter()
        .map(|turn| wire::gateway::sessions::TranscriptTurn {
            role: turn.role,
            text: turn.content,
            ts: Some(turn.at),
        })
        .collect();
    Ok(Json(wire::gateway::sessions::TranscriptResponse {
        id: session,
        command,
        turns,
        context_window: None,
        compact_at: None,
        context_source: None,
    }))
}

async fn create(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Body(request): Body<Create>,
) -> Result<Json<HarnessSessionSummary>, ApiError> {
    Ok(Json(
        service(&gateway)?
            .create_chat(&user, request.harness, request.model, request.cwd)
            .await?,
    ))
}

#[derive(Debug, Deserialize)]
struct Posted {
    wait: Option<String>,
}

async fn prompt(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path(session): Path<String>,
    Query(posted): Query<Posted>,
    extension: Option<Extension<Arc<crate::routes::v1::Service>>>,
    Body(request): Body<SessionPostRequest>,
) -> Result<Response, ApiError> {
    if request
        .user_id
        .as_ref()
        .is_some_and(|id| id != user.as_str())
    {
        return Err(ApiError::SessionForbidden);
    }
    match posted.wait.as_deref() {
        None => accept(gateway, user, session, request).await,
        Some("1") => waited(gateway, extension, session, request).await,
        Some(_) => Err(ApiError::BadRequest("wait must be 1".into())),
    }
}

async fn accept(
    gateway: Arc<Gateway>,
    user: UserId,
    session: String,
    request: SessionPostRequest,
) -> Result<Response, ApiError> {
    service(&gateway)?
        .chat(&user, &session)?
        .prompt(request.text)
        .await?;
    Ok(Json(SessionPostAccepted {
        accepted: true,
        session,
    })
    .into_response())
}

async fn waited(
    gateway: Arc<Gateway>,
    extension: Option<Extension<Arc<crate::routes::v1::Service>>>,
    session: String,
    request: SessionPostRequest,
) -> Result<Response, ApiError> {
    let runner = gateway
        .turn_runner
        .clone()
        .ok_or_else(|| ApiError::Unavailable("turn runner is unavailable".into()))?;
    let Extension(service) =
        extension.ok_or_else(|| ApiError::Unavailable("model service is unavailable".into()))?;
    let model = service.model().to_owned();
    if model.is_empty() {
        return Err(ApiError::BadRequest("model is empty".into()));
    }
    let turn = headless_turn(&model, &request, &session, service.limits())?;
    let result = run_turn(runner, turn, service.child_token()).await?;
    Ok(Json(SessionPostReply {
        message: assistant_message(&session, &result),
    })
    .into_response())
}

fn turn_messages(request: &SessionPostRequest) -> Result<Vec<Message>, ApiError> {
    if request.text.is_empty() {
        return Err(ApiError::BadRequest("user text is empty".into()));
    }
    let mut messages = Vec::new();
    if let Some(prompt) = request.system_prompt.as_deref() {
        if prompt.is_empty() {
            return Err(ApiError::BadRequest("system prompt is empty".into()));
        }
        messages.push(text_message(Role::System, prompt));
    }
    messages.push(text_message(Role::User, &request.text));
    Ok(messages)
}

fn text_message(role: Role, text: &str) -> Message {
    Message {
        role,
        content: MessageContent::Text(text.to_owned()),
        reasoning: None,
        tool_calls: None,
        tool_call_id: None,
    }
}

fn headless_turn(
    model: &str,
    request: &SessionPostRequest,
    session: &str,
    limits: TurnLimits,
) -> Result<Turn, ApiError> {
    Ok(Turn {
        request: ModelRequest {
            options: kernel::model::ModelOptions::default(),
            model: model.to_owned(),
            messages: turn_messages(request)?,
            tools: Vec::new(),
            thinking: None,
            max_tokens: None,
        },
        context: HookContextBase {
            agent_id: None,
            session_id: Some(session.to_owned()),
            timestamp: Utc::now(),
            metadata: wire::JsonObject::default(),
        },
        tool_context: ToolContext {
            agent_id: None,
            working_dir: None,
            session: None,
        },
        authorized_tools: std::collections::BTreeSet::new(),
        excluded_tools: excluded_names(request.exclude_tools.as_deref()),
        limits,
    })
}

async fn run_turn(
    runner: Arc<dyn TurnRunner>,
    turn: Turn,
    cancel: CancellationToken,
) -> Result<TurnResult, ApiError> {
    let (sender, mut events) = mpsc::channel(32);
    let running = runner.run(turn, sender, cancel);
    let drained = async { while events.recv().await.is_some() {} };
    let (result, ()) = tokio::join!(running, drained);
    result.map_err(|error| map_turn(&error))
}

fn assistant_message(session: &str, result: &TurnResult) -> SessionMessage {
    SessionMessage {
        id: Uuid::new_v4().to_string(),
        session_id: session.to_owned(),
        role: SessionMessageRole::Assistant,
        text: result.response.clone(),
        ts: Utc::now(),
        usage: None,
        model: None,
        duration_ms: None,
        thinking: None,
        tools: None,
    }
}

fn map_turn(error: &TurnError) -> ApiError {
    match error {
        TurnError::Cancelled | TurnError::Failed(_) => ApiError::Unavailable(error.to_string()),
        TurnError::Timeout => ApiError::RequestTimeout,
    }
}

async fn stop(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path(session): Path<String>,
) -> Result<Json<Accepted>, ApiError> {
    service(&gateway)?.chat(&user, &session)?.stop().await?;
    Ok(Json(Accepted { ok: true }))
}
