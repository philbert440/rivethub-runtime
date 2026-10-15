use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use wire::gateway::sessions::{SessionPostAccepted, SessionPostRequest};
use wire::harness::{HarnessId, HarnessSessionSummary};

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

async fn prompt(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path(session): Path<String>,
    Body(request): Body<SessionPostRequest>,
) -> Result<Json<SessionPostAccepted>, ApiError> {
    if request
        .user_id
        .as_ref()
        .is_some_and(|id| id != user.as_str())
    {
        return Err(ApiError::SessionForbidden);
    }
    service(&gateway)?
        .chat(&user, &session)?
        .prompt(request.text)
        .await?;
    Ok(Json(SessionPostAccepted {
        accepted: true,
        session,
    }))
}

async fn stop(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path(session): Path<String>,
) -> Result<Json<Accepted>, ApiError> {
    service(&gateway)?.chat(&user, &session)?.stop().await?;
    Ok(Json(Accepted { ok: true }))
}
