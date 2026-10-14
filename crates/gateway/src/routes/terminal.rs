use std::sync::Arc;

use axum::extract::{State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use wire::gateway::term::{PtyList, TermConfig, TermSpawnRequest, TermSpawnResponse};

use super::sessions::Accepted;
use crate::Gateway;
use crate::error::ApiError;
use crate::extract::{Body, Query};
use crate::tenancy::UserId;
use crate::term::TerminalService;

#[derive(Debug, Deserialize)]
struct Selection {
    id: Option<String>,
    session: Option<String>,
}

pub(crate) fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route("/term", get(upgrade).post(spawn).delete(kill))
        .route("/term/config", get(config))
        .route("/term/list", get(list))
        .route("/term/inject", post(inject))
        .route("/api/terminal", get(upgrade).post(spawn).delete(kill))
        .route("/api/terminal/ws", get(upgrade))
        .route("/api/terminal/config", get(config))
        .route("/api/terminal/list", get(list))
        .route("/api/terminal/inject", post(inject))
        .merge(crate::routes::chat::router())
}

pub(crate) fn service(gateway: &Gateway) -> Result<Arc<TerminalService>, ApiError> {
    gateway
        .terminal
        .clone()
        .ok_or_else(|| ApiError::Unavailable("terminals disabled".into()))
}

async fn spawn(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    body: Result<Body<TermSpawnRequest>, ApiError>,
) -> Result<(axum::http::StatusCode, Json<TermSpawnResponse>), ApiError> {
    let Body(request) = body.map_err(|error| {
        if matches!(error, ApiError::BodyMustBeObject) {
            ApiError::BadRequest("expected an object".into())
        } else {
            error
        }
    })?;
    Ok((
        axum::http::StatusCode::CREATED,
        Json(service(&gateway)?.spawn(&user, request).await?),
    ))
}

async fn kill(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Query(query): Query<Selection>,
) -> Result<Json<Accepted>, ApiError> {
    service(&gateway)?
        .kill(
            &user,
            query
                .id
                .as_deref()
                .ok_or(ApiError::NotFound("unknown pty"))?,
        )
        .await?;
    Ok(Json(Accepted { ok: true }))
}

async fn config(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
) -> Result<Json<TermConfig>, ApiError> {
    Ok(Json(service(&gateway)?.config(&user)))
}

async fn list(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
) -> Result<Json<PtyList>, ApiError> {
    Ok(Json(PtyList {
        ptys: service(&gateway)?.list(&user),
    }))
}

async fn inject(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Body(request): Body<wire::gateway::term::TermInjectRequest>,
) -> Result<Json<wire::gateway::term::TermInjectResponse>, ApiError> {
    gateway.sessions.authorize(&user, &request.session)?;
    Err(ApiError::Conflict(
        "chat messages use turn-end handoff; terminal injection is disabled".into(),
    ))
}

async fn upgrade(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Query(query): Query<Selection>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let service = service(&gateway)?;
    let (_, handle) = service.find(&user, query.id.as_deref(), query.session.as_deref())?;
    Ok(upgrade
        .on_upgrade(move |socket| async move {
            if let Err(error) = crate::term::socket::run(socket, service, handle).await {
                tracing::debug!(%error, "terminal websocket ended");
            }
        })
        .into_response())
}
