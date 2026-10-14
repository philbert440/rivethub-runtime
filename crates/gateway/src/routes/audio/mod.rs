mod bridge;
mod mic;
mod voice;

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use axum::routing::any;
use axum::routing::get;
use wire::gateway::GatewayError;

use crate::Gateway;

pub(crate) use bridge::AudioBridge;
pub use bridge::AudioConfig;
pub use voice::VoiceConfigError;
pub(crate) use voice::VoiceProxy;
pub use voice::VoiceSettings;

pub(crate) fn mount(router: Router<Arc<Gateway>>) -> Router<Arc<Gateway>> {
    router
        .route("/audio", get(status).head(status).fallback(known_fallback))
        .route(
            "/audio/status",
            get(status).head(status).fallback(known_fallback),
        )
        .route(
            "/audio/health",
            get(health).head(health).fallback(known_fallback),
        )
        .route("/audio/mic", any(mic::socket))
        .route("/audio/{*rest}", any(nested))
        .route(
            "/api/audio",
            get(status).head(status).fallback(known_fallback),
        )
        .route(
            "/api/audio/status",
            get(status).head(status).fallback(known_fallback),
        )
        .route(
            "/api/audio/health",
            get(health).head(health).fallback(known_fallback),
        )
        .route("/api/audio/mic", any(mic::socket))
        .route("/api/audio/{*rest}", any(nested))
        .merge(voice::router())
}

pub(crate) fn disabled_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(
            GatewayError::new("audio mic bridge disabled on this node")
                .hint("set RIVETOS_DEN_AUDIO=1"),
        ),
    )
        .into_response()
}

pub(crate) fn not_found_response() -> Response {
    (StatusCode::NOT_FOUND, Json(GatewayError::new("not found"))).into_response()
}

fn unavailable_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(GatewayError::new("audio bridge unavailable")),
    )
        .into_response()
}

async fn status(State(gateway): State<Arc<Gateway>>) -> Response {
    let Some(bridge) = gateway.audio() else {
        return disabled_response();
    };
    match bridge.core().report().await {
        Ok(body) => (StatusCode::OK, Json(body)).into_response(),
        Err(bridge::AudioFault::Unavailable) => unavailable_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(GatewayError::new(error.to_string())),
        )
            .into_response(),
    }
}

async fn health(State(gateway): State<Arc<Gateway>>) -> Response {
    let Some(bridge) = gateway.audio() else {
        return disabled_response();
    };
    match bridge.core().health().await {
        Ok(body) if body.ok => (StatusCode::OK, Json(body)).into_response(),
        Ok(body) => (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response(),
        Err(bridge::AudioFault::Unavailable) => unavailable_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(GatewayError::new(error.to_string())),
        )
            .into_response(),
    }
}

async fn known_fallback(State(gateway): State<Arc<Gateway>>) -> Response {
    if gateway.audio().is_none() {
        disabled_response()
    } else {
        super::method_not_allowed().await.into_response()
    }
}

async fn nested(State(gateway): State<Arc<Gateway>>) -> Response {
    if gateway.audio().is_none() {
        disabled_response()
    } else {
        not_found_response()
    }
}
