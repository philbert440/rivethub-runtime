mod body;
mod catalog;
mod change;
mod create;
mod error;
mod present;
mod read;
mod wait;

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::routing::post;

use crate::Gateway;
use error::TaskHttpError;
pub(crate) use present::status_out;

pub const WAIT_FLOOR_MS: u64 = 1;
pub const WAIT_CEILING_MS: u64 = 120_000;
pub const WAIT_DEFAULT_MS: u64 = 30_000;

pub(crate) fn router() -> Router<Arc<Gateway>> {
    Router::new().nest("/api/tasks", task_routes()).route(
        "/api/catalog/agents",
        get(catalog::agents).fallback(method_not_allowed),
    )
}

fn task_routes() -> Router<Arc<Gateway>> {
    Router::new()
        .route(
            "/",
            get(read::list)
                .post(create::create)
                .fallback(method_not_allowed)
                .layer(DefaultBodyLimit::max(super::BODY_LIMIT)),
        )
        .route("/{id}", get(read::get_one).fallback(method_not_allowed))
        .route("/{id}/wait", get(read::wait).fallback(method_not_allowed))
        .route(
            "/{id}/kill",
            post(change::kill).fallback(method_not_allowed),
        )
        .fallback(not_found)
}

async fn method_not_allowed() -> TaskHttpError {
    TaskHttpError::MethodNotAllowed
}

async fn not_found() -> TaskHttpError {
    TaskHttpError::NotFound("no such task route".to_owned())
}
