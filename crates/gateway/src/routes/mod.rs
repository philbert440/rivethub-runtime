pub mod agents;
pub mod capture;
pub mod capture_schema;
pub mod chat;
pub mod devices;
pub mod files;
pub mod harnesses;
pub mod memory;
pub mod sessions;
pub mod tags;
pub mod terminal;
pub mod v1;
pub mod wiki;

use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use wire::gateway::HealthzResponse;

use crate::Gateway;
use crate::error::ApiError;

pub(crate) const BODY_LIMIT: usize = 256 * 1024;

pub(crate) fn router() -> axum::Router<Arc<Gateway>> {
    harnesses::mount(files::mount(wiki::router()))
        .nest("/api/agents", agents::router())
        .merge(devices::router())
}

pub(crate) async fn healthz(State(gateway): State<Arc<Gateway>>) -> Json<HealthzResponse> {
    Json(HealthzResponse {
        ok: true,
        sessions: gateway.sessions().count() as u64,
        name: gateway.settings().hostname.clone(),
        node: gateway.settings().node.clone(),
    })
}

pub(crate) async fn not_found(
    State(gateway): State<Arc<Gateway>>,
    request: axum::extract::Request,
) -> axum::response::Response {
    use tower::ServiceExt as _;

    match v1::router().with_state(gateway).oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

pub(crate) async fn method_not_allowed() -> ApiError {
    ApiError::MethodNotAllowed
}
