use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use wire::gateway::GatewayError;
use wire::harness::HarnessId;
use wire::task::TaskWaitTimeoutResponse;
use wire::task::TaskWire;

pub(super) const WAIT_LEFT: &str = "wait deadline exceeded";
pub(super) const WAIT_KILLED: &str = "wait deadline exceeded — task killed";

#[derive(Debug, thiserror::Error)]
pub(super) enum TaskHttpError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("harness \"{0}\" is not allowed on this node (den.allowed_harnesses)")]
    HarnessDenied(HarnessId),
    #[error("{0}")]
    Failed(String),
    #[error("internal error")]
    Internal,
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("body too large")]
    PayloadTooLarge,
    #[error("{message}")]
    WaitExceeded {
        task: Option<Box<TaskWire>>,
        message: &'static str,
    },
}

impl TaskHttpError {
    pub(super) fn wait_left() -> Self {
        Self::WaitExceeded {
            task: None,
            message: WAIT_LEFT,
        }
    }

    pub(super) fn wait_killed(task: Option<TaskWire>) -> Self {
        Self::WaitExceeded {
            task: task.map(Box::new),
            message: WAIT_KILLED,
        }
    }
}

impl IntoResponse for TaskHttpError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, GatewayError::new(message)),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, GatewayError::new(message)),
            Self::Conflict(message) => (StatusCode::CONFLICT, GatewayError::new(message)),
            Self::HarnessDenied(harness) => {
                let mut body = GatewayError::new(format!(
                    "harness \"{harness}\" is not allowed on this node (den.allowed_harnesses)"
                ))
                .code("harness_not_allowed");
                body.harness_id = Some(harness);
                return (StatusCode::FORBIDDEN, Json(body)).into_response();
            }
            Self::Failed(message) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GatewayError::new(message),
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GatewayError::new("internal error"),
            ),
            Self::MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                GatewayError::new("method not allowed"),
            ),
            Self::PayloadTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                GatewayError::new("body too large"),
            ),
            Self::WaitExceeded { message, task } => {
                return (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(TaskWaitTimeoutResponse {
                        error: message.to_owned(),
                        task: task.map(|boxed| *boxed),
                    }),
                )
                    .into_response();
            }
        };
        (status, Json(body)).into_response()
    }
}
