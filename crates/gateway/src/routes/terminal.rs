use std::sync::Arc;

use axum::extract::{FromRequestParts, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use wire::gateway::term::{PtyList, TermConfig, TermSpawnRequest, TermSpawnResponse};

use super::sessions::Accepted;
use crate::Gateway;
use crate::error::ApiError;
use crate::extract::{Body, Query};
use crate::routes::agents::Failure;
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
        .route("/term/{*rest}", any(missing))
        .merge(crate::routes::chat::router())
}

pub(crate) fn service(gateway: &Gateway) -> Result<Arc<TerminalService>, ApiError> {
    opened(gateway, "terminals disabled")
}

pub(crate) fn require(gateway: &Gateway) -> Result<Arc<TerminalService>, ApiError> {
    opened(gateway, "terminal disabled")
}

fn opened(gateway: &Gateway, message: &str) -> Result<Arc<TerminalService>, ApiError> {
    gateway
        .terminal
        .clone()
        .ok_or_else(|| ApiError::Unavailable(message.into()))
}

async fn spawn(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    body: Result<Body<TermSpawnRequest>, ApiError>,
) -> Result<(axum::http::StatusCode, Json<TermSpawnResponse>), Failure> {
    let service = require(&gateway)?;
    let Body(request) = body.map_err(|error| {
        if matches!(error, ApiError::BodyMustBeObject) {
            ApiError::BadRequest("expected an object".into())
        } else {
            error
        }
    })?;
    let preset = match request.agent_id.clone() {
        Some(id) => Some(crate::routes::agents::launch_preset(&gateway, id).await?),
        None => None,
    };
    Ok((
        axum::http::StatusCode::CREATED,
        Json(service.spawn_from(&user, request, preset).await?),
    ))
}

async fn kill(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Query(query): Query<Selection>,
) -> Result<Json<Accepted>, ApiError> {
    require(&gateway)?
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

async fn config(State(gateway): State<Arc<Gateway>>, user: UserId) -> Json<TermConfig> {
    let document = match gateway.terminal.clone() {
        Some(service) => service.config(&user),
        None => TermConfig::disabled(config::TermSettings::defaults().max_ptys as u64),
    };
    Json(document)
}

async fn list(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
) -> Result<Json<PtyList>, ApiError> {
    Ok(Json(PtyList {
        ptys: require(&gateway)?.list(&user),
    }))
}

async fn inject(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Body(request): Body<wire::gateway::term::TermInjectRequest>,
) -> Result<Json<wire::gateway::term::TermInjectResponse>, ApiError> {
    let service = require(&gateway)?;
    gateway.sessions.authorize(&user, &request.session)?;
    let pty_id = service.inject(&user, request).await?;
    Ok(Json(wire::gateway::term::TermInjectResponse {
        ok: true,
        pty_id,
        dismissed_dialog: None,
    }))
}

async fn missing(State(gateway): State<Arc<Gateway>>) -> Response {
    match require(&gateway) {
        Ok(service) => {
            drop(service);
            ApiError::NotFound("not found").into_response()
        }
        Err(error) => error.into_response(),
    }
}

async fn upgrade(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Query(query): Query<Selection>,
    request: axum::extract::Request,
) -> Response {
    let service = match require(&gateway) {
        Ok(service) => service,
        Err(error) => return error.into_response(),
    };
    let (mut parts, body) = request.into_parts();
    drop(body);
    let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => {
            tracing::debug!(%rejection, "terminal upgrade rejected");
            return rejection.into_response();
        }
    };
    let found = service.find(&user, query.id.as_deref(), query.session.as_deref());
    let (_, handle) = match found {
        Ok(pair) => pair,
        Err(error) => return error.into_response(),
    };
    upgrade
        .on_upgrade(move |socket| async move {
            if let Err(error) = crate::term::socket::run(socket, service, handle).await {
                tracing::debug!(%error, "terminal websocket ended");
            }
        })
        .into_response()
}
