use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use wire::gateway::GatewayError;
use wire::task::TaskWaitTimeoutResponse;
use wire::task::TaskWire;

use crate::tasks::Refusal;
use crate::tasks::SubmitError;

pub(super) const WAIT_LEFT: &str = "wait deadline exceeded";

#[derive(Debug, thiserror::Error)]
pub(super) enum TaskHttpError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Failed(String),
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("body too large")]
    PayloadTooLarge,
    #[error("wait deadline exceeded")]
    WaitExceeded(Box<TaskWire>),
}

impl TaskHttpError {
    pub(super) fn wait_left(task: TaskWire) -> Self {
        Self::WaitExceeded(Box::new(task))
    }
}

impl From<SubmitError> for TaskHttpError {
    fn from(error: SubmitError) -> Self {
        let message = error.to_string();
        match error {
            SubmitError::Refused(Refusal::Unknown(_) | Refusal::NotOnNode { .. }) => {
                Self::NotFound(message)
            }
            SubmitError::Refused(Refusal::Offline(_)) | SubmitError::Unavailable(_) => {
                Self::Unavailable(message)
            }
            SubmitError::Refused(
                Refusal::AgentRequired
                | Refusal::EmptyNode
                | Refusal::ConflictingNode { .. }
                | Refusal::Ambiguous(_)
                | Refusal::PresetHarness(_)
                | Refusal::NotHarnessed(_),
            ) => Self::BadRequest(message),
            SubmitError::Depth(_) => Self::Conflict(message),
            SubmitError::Store(error) => super::body::store_error(&error),
        }
    }
}

impl IntoResponse for TaskHttpError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, GatewayError::new(message)),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, GatewayError::new(message)),
            Self::Conflict(message) => (StatusCode::CONFLICT, GatewayError::new(message)),
            Self::Unavailable(message) => {
                (StatusCode::SERVICE_UNAVAILABLE, GatewayError::new(message))
            }
            Self::Failed(message) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GatewayError::new(message),
            ),
            Self::MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                GatewayError::new("method not allowed"),
            ),
            Self::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                GatewayError::new("body too large"),
            ),
            Self::WaitExceeded(task) => {
                return (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(TaskWaitTimeoutResponse {
                        error: WAIT_LEFT.to_owned(),
                        task: Some(*task),
                    }),
                )
                    .into_response();
            }
        };
        (status, Json(body)).into_response()
    }
}
