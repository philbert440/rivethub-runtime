mod chat;
mod conversations;
mod error;
mod storage;
mod streaming;

pub use error::Error;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use kernel::agent::{TurnLimits, TurnRunner};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use wire::gateway::v1::{Model, Models};

use crate::Gateway;
use crate::error::ApiError;

#[expect(
    missing_debug_implementations,
    reason = "the turn runner port does not implement Debug"
)]
pub struct BoundAgent {
    pub runner: Arc<dyn TurnRunner>,
    pub model: String,
}

#[expect(
    missing_debug_implementations,
    reason = "the turn runner port does not implement Debug"
)]
pub struct Service {
    model: String,
    roster: BTreeMap<String, BoundAgent>,
    conversations: storage::Store,
    limits: TurnLimits,
    cancel: CancellationToken,
    tasks: TaskTracker,
}

impl Service {
    pub async fn open(
        model: String,
        directory: &Path,
        cancel: CancellationToken,
        roster: BTreeMap<String, BoundAgent>,
    ) -> Result<Arc<Self>, Error> {
        let tasks = TaskTracker::new();
        let conversations = storage::Store::open(directory, &tasks, &cancel).await?;
        Ok(Arc::new(Self {
            model,
            roster,
            conversations,
            limits: TurnLimits {
                steps: 1,
                timeout: Duration::from_secs(120),
                cleanup_timeout: Duration::from_secs(5),
                response_bytes: 4 * 1024 * 1024,
            },
            cancel,
            tasks,
        }))
    }

    fn resolve(&self, id: &str) -> Result<(Arc<dyn TurnRunner>, String), Error> {
        if self.roster.is_empty() {
            return Err(Error::Unavailable);
        }
        let Some(agent) = self.roster.get(id) else {
            let registered = self
                .roster
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",");
            return Err(Error::Invalid(format!(
                "Unknown agent: \"{id}\". Registered: [{registered}]"
            )));
        };
        Ok((Arc::clone(&agent.runner), agent.model.clone()))
    }

    pub(crate) fn model(&self) -> &str {
        &self.model
    }

    pub(crate) fn limits(&self) -> TurnLimits {
        self.limits
    }

    pub(crate) fn child_token(&self) -> CancellationToken {
        self.cancel.child_token()
    }

    pub fn router(self: &Arc<Self>, gateway: &Arc<Gateway>, secure: bool) -> Router {
        gateway.router(secure).layer(Extension(Arc::clone(self)))
    }

    pub async fn shutdown(&self) {
        self.cancel.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}

pub(super) fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route(
            "/v1/models",
            get(models).fallback(crate::routes::method_not_allowed),
        )
        .route(
            "/v1/chat/completions",
            post(chat::complete).fallback(crate::routes::method_not_allowed),
        )
        .route(
            "/api/conversations",
            get(conversations::list)
                .post(conversations::create)
                .fallback(crate::routes::method_not_allowed),
        )
        .route(
            "/api/conversations/{id}",
            get(conversations::get)
                .patch(conversations::rename)
                .delete(conversations::delete)
                .fallback(crate::routes::method_not_allowed),
        )
        .fallback(|| async { ApiError::NotFound("not found") })
        .layer(axum::extract::DefaultBodyLimit::max(
            crate::routes::BODY_LIMIT,
        ))
}

fn body<T>(result: Result<Json<T>, axum::extract::rejection::JsonRejection>) -> Result<T, Error> {
    result.map(|Json(body)| body).map_err(|error| {
        if error.status() == axum::http::StatusCode::PAYLOAD_TOO_LARGE {
            Error::BodyTooLarge
        } else {
            Error::Invalid(error.body_text())
        }
    })
}

fn service(extension: Option<Extension<Arc<Service>>>) -> Result<Arc<Service>, Error> {
    let Extension(service) = extension.ok_or(Error::Unavailable)?;
    if service.cancel.is_cancelled() {
        return Err(Error::Unavailable);
    }
    Ok(service)
}

async fn models(extension: Option<Extension<Arc<Service>>>) -> Result<Json<Models>, Error> {
    let service = service(extension)?;
    Ok(Json(Models {
        data: service
            .roster
            .keys()
            .map(|id| Model {
                id: id.clone(),
                created: 0,
                owned_by: "rivetos".to_owned(),
            })
            .collect(),
    }))
}
