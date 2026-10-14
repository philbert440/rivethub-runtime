use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use wire::gateway::v1::{ErrorDetail, ErrorKind, ErrorResponse};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("conversation not found")]
    NotFound,
    #[error("v1 service unavailable")]
    Unavailable,
    #[error("request cancelled")]
    Cancelled,
    #[error("conversation has an active turn")]
    Busy,
    #[error("request body too large")]
    BodyTooLarge,
    #[error("provider failed: {0}")]
    Provider(#[from] kernel::model::ModelProviderError),
    #[error("agent turn failed: {0}")]
    Turn(#[from] agent_loop::Error),
    #[error("conversation IO failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("conversation JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("stream serialization failed: {0}")]
    Stream(#[from] axum::Error),
    #[error("task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("token usage overflow")]
    UsageOverflow,
}

impl Error {
    pub(super) fn payload(&self) -> (StatusCode, ErrorResponse) {
        let (status, kind, message) = match self {
            Self::Invalid(message) => (
                StatusCode::BAD_REQUEST,
                ErrorKind::InvalidRequestError,
                message.clone(),
            ),
            Self::NotFound => (
                StatusCode::NOT_FOUND,
                ErrorKind::InvalidRequestError,
                self.to_string(),
            ),
            Self::Busy => (
                StatusCode::CONFLICT,
                ErrorKind::InvalidRequestError,
                self.to_string(),
            ),
            Self::BodyTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorKind::InvalidRequestError,
                self.to_string(),
            ),
            Self::Unavailable | Self::Cancelled => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorKind::ServerError,
                self.to_string(),
            ),
            Self::Provider(_) | Self::Turn(_) => (
                StatusCode::BAD_GATEWAY,
                ErrorKind::ServerError,
                "agent turn failed".to_owned(),
            ),
            Self::Io(_) | Self::Json(_) | Self::Stream(_) | Self::Task(_) | Self::UsageOverflow => {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorKind::ServerError,
                    "internal error".to_owned(),
                )
            }
        };
        (
            status,
            ErrorResponse {
                error: ErrorDetail { message, kind },
            },
        )
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, body) = self.payload();
        tracing::debug!(error = %self, "v1 request failed");
        (status, Json(body)).into_response()
    }
}
