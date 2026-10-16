pub mod agents;
pub mod audio;
pub mod ca;
pub mod capture;
pub mod capture_schema;
pub mod chat;
pub mod devices;
pub mod files;
pub mod harnesses;
pub mod memory;
pub mod sessions;
pub mod tags;
pub mod tasks;
pub mod terminal;
pub mod v1;
pub mod wiki;

mod surface;

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use kernel::memory::MemoryStoreError;
use wire::gateway::HealthzResponse;

use crate::Gateway;
use crate::error::ApiError;

pub(crate) const BODY_LIMIT: usize = 256 * 1024;

pub(crate) use surface::MeshCache;
pub(crate) use surface::plain_get;
pub(crate) use surface::rewrite_request;

pub(crate) fn router() -> axum::Router<Arc<Gateway>> {
    audio::mount(harnesses::mount(files::mount(wiki::router())))
        .nest("/api/agents", agents::router())
        .merge(devices::router())
        .merge(ca::router())
}

pub(crate) fn mount(router: Router<Arc<Gateway>>) -> Router<Arc<Gateway>> {
    surface::mount(router)
}

pub(crate) fn join_task<T>(joined: Result<T, tokio::task::JoinError>) -> Result<T, ApiError> {
    match joined {
        Ok(value) => Ok(value),
        Err(error) => {
            tracing::error!("blocking task failed: {error}");
            Err(ApiError::Task(error))
        }
    }
}

pub(crate) async fn healthz(State(gateway): State<Arc<Gateway>>) -> axum::response::Response {
    let sessions = gateway.sessions().count() as u64;
    let name = gateway.settings().hostname.clone();
    let node = gateway.settings().node.clone();
    match gateway.store().probe().await {
        Ok(()) => (
            StatusCode::OK,
            Json(HealthzResponse {
                ok: true,
                sessions,
                name,
                node,
                degraded: None,
            }),
        )
            .into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthzResponse {
                ok: false,
                sessions,
                name,
                node,
                degraded: Some(degraded_reason(&error)),
            }),
        )
            .into_response(),
    }
}

fn degraded_reason(error: &MemoryStoreError) -> String {
    let detail = match error {
        MemoryStoreError::Unavailable(source) => source.to_string(),
        MemoryStoreError::Query { message, .. } | MemoryStoreError::InvalidRequest { message } => {
            message.clone()
        }
        MemoryStoreError::TagTablesMissing => error.to_string(),
    };
    format!("database unreachable: {detail}")
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
