mod body;
mod catalog;
mod change;
mod create;
mod error;
mod job;
mod outcomes;
mod present;
mod read;
mod wait;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::routing::post;
use kernel::tasks::ApprovalBroker;
use kernel::tasks::CatalogSource;
use kernel::tasks::TaskAgents;
use kernel::tasks::TaskJobs;
use kernel::tasks::TaskQuery;
use kernel::tasks::TaskStore;
use kernel::tasks::TaskWatch;
use wire::harness::HarnessId;

use error::TaskHttpError;

pub const COMMAND_LIST_TIMEOUT: Duration = Duration::from_millis(3_000);
pub const WAIT_FLOOR_MS: u64 = 1;
pub const WAIT_CEILING_MS: u64 = 120_000;
pub const WAIT_DEFAULT_MS: u64 = 30_000;

#[derive(Debug)]
pub struct TasksApi {
    pub store: Arc<dyn TaskStore>,
    pub jobs: Arc<dyn TaskJobs>,
    pub query: Arc<dyn TaskQuery>,
    pub watch: Arc<dyn TaskWatch>,
    pub catalog: Arc<dyn CatalogSource>,
    pub agents: Arc<dyn TaskAgents>,
    pub approvals: Option<Arc<dyn ApprovalBroker>>,
    pub node: String,
    pub mesh: bool,
    pub allowed_harnesses: Option<Vec<HarnessId>>,
    pub command_timeout: Duration,
}

pub fn router(api: Arc<TasksApi>) -> Router {
    Router::new()
        .nest("/api/tasks", task_routes())
        .nest("/api/catalog", catalog::routes())
        .nest("/api/outcomes", outcomes::routes())
        .with_state(api)
}

fn task_routes() -> Router<Arc<TasksApi>> {
    Router::new()
        .route("/", limited(get(read::list).post(create::create)))
        .route("/{id}", get(read::get_one).fallback(method_not_allowed))
        .route("/{id}/wait", get(read::wait).fallback(method_not_allowed))
        .route("/{id}/steer", limited(post(change::steer)))
        .route(
            "/{id}/kill",
            post(change::kill).fallback(method_not_allowed),
        )
        .route("/{id}/approvals", limited(post(change::missing_request)))
        .route(
            "/{id}/approvals/{request_id}",
            limited(post(change::approve)),
        )
        .fallback(method_not_allowed)
}

fn limited(
    route: axum::routing::MethodRouter<Arc<TasksApi>>,
) -> axum::routing::MethodRouter<Arc<TasksApi>> {
    route
        .fallback(method_not_allowed)
        .layer(DefaultBodyLimit::max(super::BODY_LIMIT))
}

async fn method_not_allowed() -> TaskHttpError {
    TaskHttpError::MethodNotAllowed
}
