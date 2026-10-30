use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use chrono::Utc;
use kernel::ToolContext;
use kernel::agent::{Turn, TurnError, TurnLimits, TurnResult, TurnRunner, excluded_names};
use kernel::harness::{SessionOption, TurnAttachment};
use kernel::model::ModelRequest;
use kernel::topics::DeliveredVia;
use kernel::topics::StartupContext;
use kernel::topics::render;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use wire::gateway::agents::UserTurnAttachment;
use wire::gateway::sessions::{
    MessageUsage, SessionMessage, SessionMessageRole, SessionPostAccepted, SessionPostReply,
    SessionPostRequest, SessionSteerRequest, SessionSteerResponse,
};
use wire::harness::{
    ApprovalDecision, HarnessId, HarnessSessionCreateRequest, HarnessSessionSummary, SteerOutcome,
};
use wire::hooks::HookContextBase;
use wire::message::{Message, MessageContent, Role};

use super::sessions::Accepted;
use super::terminal::{require, service};
use crate::Gateway;
use crate::error::ApiError;
use crate::extract::Body;
use crate::routes::agents::Failure;
use crate::tenancy::UserId;
use crate::term::{AgentLaunch, ChatStart, Delegate, DelegatedTurn};
use crate::uploads::AttachedTurn;

pub(crate) fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route("/api/harness-sessions", post(create).get(list))
        .route(
            "/term/harness-sessions/{session}/transcript",
            get(transcript),
        )
        .route(
            "/api/terminal/harness-sessions/{session}/transcript",
            get(transcript),
        )
        .route("/api/harness-sessions/{session}/turn", post(prompt))
        .route("/api/harness-sessions/{session}/steer", post(steer))
        .route("/api/harness-sessions/{session}/stop", post(stop))
        .route(
            "/api/harness-sessions/{session}/approvals/{request}",
            post(approve),
        )
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
    gateway.sessions.authorize_visible(&user, &session)?;
    let reader = gateway
        .transcript
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("transcript reader unavailable".into()))?;
    let canonical = session
        .parse::<wire::session_id::SessionId>()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let command = canonical.harness().to_string();
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
    Body(request): Body<HarnessSessionCreateRequest>,
) -> Result<Json<HarnessSessionSummary>, Failure> {
    let service = service(&gateway)?;
    let topic = match request.topic_id.as_deref() {
        Some(key) => Some(super::topics::resolve(gateway.topics()?.as_ref(), key).await?),
        None => None,
    };
    let context = match &topic {
        Some(topic) => super::topics::prepare(&gateway, topic, request.prompt.as_deref()).await,
        None => None,
    };
    let rendered = topic
        .as_ref()
        .zip(context.as_ref())
        .and_then(|(topic, context)| render(context, topic.name.as_str()));
    let agent = match request.agent_id {
        Some(id) => Some(crate::routes::agents::launch_preset(&gateway, id).await?),
        None => None,
    };
    let harness = request
        .harness
        .or_else(|| agent.as_ref().and_then(|agent| agent.harness))
        .ok_or_else(|| ApiError::BadRequest("harness is required".into()))?;
    let delivered = match (&rendered, service.adapter()) {
        (Some(_), Some(adapter)) => adapter.system_prompt(harness).delivered_via(),
        _ => DeliveredVia::None,
    };
    let start = ChatStart {
        harness,
        model: request.model,
        effort: None,
        cwd: request.cwd.map(std::path::PathBuf::from),
        agent,
        startup_context: rendered,
    };
    let mut summary = service.start_chat(&user, start).await?;
    if let Some(topic) = topic {
        let session = summary.session_id.clone();
        summary.startup_context = super::topics::begin(
            &gateway,
            &service,
            &user,
            super::topics::Started {
                session: &session,
                owner: session.as_str(),
                topic: &topic,
                context: context.map(|context| StartupContext {
                    delivered_via: delivered,
                    ..context
                }),
            },
        )
        .await?;
    }
    Ok(Json(summary))
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
) -> Result<Response, Failure> {
    if request
        .user_id
        .as_ref()
        .is_some_and(|id| id != user.as_str())
    {
        return Err(ApiError::SessionForbidden.into());
    }
    match posted.wait.as_deref() {
        None => Ok(accept(gateway, user, session, request).await?),
        Some("1") if request.harness.is_some() || delegate_of(&gateway, &request).is_some() => {
            delegated(gateway, user, session, request).await
        }
        Some("1") => Ok(waited(gateway, extension, session, request).await?),
        Some(_) => Err(ApiError::BadRequest("wait must be 1".into()).into()),
    }
}

async fn accept(
    gateway: Arc<Gateway>,
    user: UserId,
    session: String,
    request: SessionPostRequest,
) -> Result<Response, ApiError> {
    let attachments = staged(&gateway, &user, request.attachments).await?;
    let chat = service(&gateway)?.chat(&user, &session)?;
    let options = SessionOption::for_turn(request.model, request.thinking);
    if !options.is_empty() {
        admit(&gateway, &session, &options)?;
    }
    if !attachments.is_empty() {
        let sent = crate::sessions::control::with_system_prompt(
            request.system_prompt.as_deref(),
            request.text.clone(),
        );
        journal(&gateway, &session, &sent, &attachments).await?;
    }
    chat.prompt(request.text, request.system_prompt, attachments, options)
        .await?;
    Ok(Json(SessionPostAccepted {
        accepted: true,
        session,
    })
    .into_response())
}

async fn journal(
    gateway: &Gateway,
    session: &str,
    text: &str,
    attachments: &[TurnAttachment],
) -> Result<(), ApiError> {
    let canonical = session
        .parse::<wire::session_id::SessionId>()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    super::harnesses::open_store(gateway)
        .await?
        .record_turn(&canonical, AttachedTurn::new(text, attachments))
        .await
        .map_err(super::harnesses::upload_error)
}

async fn steer(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path(session): Path<String>,
    Body(request): Body<SessionSteerRequest>,
) -> Result<Json<SessionSteerResponse>, ApiError> {
    let attachments = staged(&gateway, &user, request.attachments).await?;
    let chat = service(&gateway)?.chat(&user, &session)?;
    let outcome = chat
        .steer(request.text.clone(), attachments.clone())
        .await?;
    if outcome != SteerOutcome::PromptRequired && !attachments.is_empty() {
        journal(&gateway, &session, &request.text, &attachments).await?;
    }
    Ok(Json(SessionSteerResponse { outcome }))
}

fn admit(gateway: &Gateway, session: &str, options: &[SessionOption]) -> Result<(), ApiError> {
    let canonical = session
        .parse::<wire::session_id::SessionId>()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let adapter = service(gateway)?
        .adapter()
        .ok_or_else(|| ApiError::Unavailable("chat harness unavailable".into()))?;
    for option in options {
        adapter.admit(&canonical, option)?;
    }
    Ok(())
}

async fn staged(
    gateway: &Gateway,
    user: &UserId,
    attachments: Option<Vec<UserTurnAttachment>>,
) -> Result<Vec<TurnAttachment>, ApiError> {
    let Some(attachments) = attachments.filter(|list| !list.is_empty()) else {
        return Ok(Vec::new());
    };
    let store = super::harnesses::open_store(gateway).await?;
    let now = Utc::now();
    let mut staged = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        let found = store
            .attachment(user, &attachment.path_or_uri, now)
            .await
            .map_err(super::harnesses::upload_error)?;
        staged.push(found);
    }
    Ok(staged)
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

async fn delegated(
    gateway: Arc<Gateway>,
    user: UserId,
    session: String,
    request: SessionPostRequest,
) -> Result<Response, Failure> {
    let delegate = delegate_of(&gateway, &request);
    let harness = request
        .harness
        .or_else(|| delegate.as_ref().map(|agent| agent.harness))
        .ok_or_else(|| ApiError::BadRequest("harness is required".into()))?;
    let configured = match (&delegate, request.agent.as_deref()) {
        (Some(agent), Some(name)) => agent.cwd.clone().map(|directory| AgentLaunch {
            name: name.to_owned(),
            directory,
            harness: Some(agent.harness),
            model: String::new(),
            effort: String::new(),
        }),
        _ => configured_preset(&gateway, &request, harness).await?,
    };
    let attachments = staged(&gateway, &user, request.attachments).await?;
    let journal = if attachments.is_empty() {
        None
    } else {
        Some(super::harnesses::open_store(&gateway).await?)
    };
    let turn = DelegatedTurn {
        start: ChatStart {
            harness,
            model: request
                .model
                .or_else(|| delegate.as_ref().and_then(|agent| agent.model.clone())),
            effort: request.thinking,
            cwd: match configured {
                Some(_) => None,
                None => request.working_dir.map(std::path::PathBuf::from),
            },
            agent: configured,
            startup_context: None,
        },
        text: request.text,
        system_prompt: request.system_prompt,
        attachments,
        journal,
        delegate,
    };
    let reply = service(&gateway)?.delegated_turn(&user, turn).await?;
    let mut message = reply_message(&session, reply.text);
    message.usage = reply.usage.map(|usage| MessageUsage {
        prompt_tokens: usage.prompt_tokens,
        completion_tokens: usage.completion_tokens,
        cached_tokens: usage
            .cached_tokens
            .or(usage.cache_read_tokens)
            .unwrap_or_default(),
    });
    Ok(Json(SessionPostReply { message }).into_response())
}

async fn configured_preset(
    gateway: &Gateway,
    request: &SessionPostRequest,
    harness: HarnessId,
) -> Result<Option<AgentLaunch>, Failure> {
    let (Some(name), Some(working)) = (request.agent.as_deref(), request.working_dir.as_deref())
    else {
        return Ok(None);
    };
    let Some(preset) = gateway
        .presets()
        .list(None)
        .await?
        .into_iter()
        .find(|preset| preset.name == name && preset.harness_id == Some(harness))
    else {
        return Ok(None);
    };
    let launch = crate::routes::agents::launch_preset(gateway, preset.id).await?;
    let requested = std::path::PathBuf::from(working);
    let home = launch.directory.clone();
    let same = tokio::task::spawn_blocking(move || {
        Ok::<_, std::io::Error>(requested.canonicalize()? == home.canonicalize()?)
    })
    .await
    .map_err(ApiError::Task)?
    .map_err(ApiError::Io)?;
    Ok(same.then_some(launch))
}

fn delegate_of(gateway: &Gateway, request: &SessionPostRequest) -> Option<Delegate> {
    let delegate = gateway.terminal()?.delegate(request.agent.as_deref()?)?;
    request
        .harness
        .is_none_or(|harness| harness == delegate.harness)
        .then(|| delegate.clone())
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
    reply_message(session, result.response.clone())
}

fn reply_message(session: &str, text: String) -> SessionMessage {
    SessionMessage {
        id: Uuid::new_v4().to_string(),
        session_id: session.to_owned(),
        role: SessionMessageRole::Assistant,
        text,
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

#[derive(Debug, Deserialize)]
struct Approval {
    decision: ApprovalDecision,
}

async fn approve(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path((segment, request)): Path<(String, String)>,
    Body(approval): Body<Approval>,
) -> Result<Json<Accepted>, ApiError> {
    let decoded = wire::session_id::decode_segment(&segment)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let wire::session_id::SessionRef::Canonical(session) = decoded.session else {
        return Err(ApiError::BadRequest("canonical session required".into()));
    };
    service(&gateway)?
        .answer_approval(&user, &session, &request, approval.decision)
        .await?;
    Ok(Json(Accepted { ok: true }))
}
