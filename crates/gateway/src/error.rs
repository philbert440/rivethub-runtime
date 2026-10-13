use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use kernel::memory::MemoryStoreError;
use wire::gateway::GatewayError;

const INVALID_JSON: &str = "invalid JSON";
const BODY_MUST_BE_OBJECT: &str = "body must be a JSON object";
const INTERNAL_ERROR: &str = "internal error";
const STORE_UNAVAILABLE: &str = "memory store unavailable";
const STORE_QUERY_FAILED: &str = "memory query failed";

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    NotFound(&'static str),
    #[error("method not allowed")]
    MethodNotAllowed,
    #[error("body too large")]
    BodyTooLarge,
    #[error("request timeout")]
    RequestTimeout,
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden origin")]
    ForbiddenOrigin,
    #[error("serialization failed: {0}")]
    Serialize(#[source] serde_json::Error),
    #[error(transparent)]
    Store(#[from] MemoryStoreError),
}

pub fn invalid_json() -> ApiError {
    ApiError::BadRequest(INVALID_JSON.to_owned())
}

pub fn body_must_be_object() -> ApiError {
    ApiError::BadRequest(BODY_MUST_BE_OBJECT.to_owned())
}

impl ApiError {
    fn status_and_body(self) -> (StatusCode, GatewayError) {
        match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, GatewayError::new(message)),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, GatewayError::new(message)),
            Self::MethodNotAllowed => (
                StatusCode::METHOD_NOT_ALLOWED,
                GatewayError::new("method not allowed"),
            ),
            Self::BodyTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                GatewayError::new("body too large"),
            ),
            Self::RequestTimeout => (
                StatusCode::REQUEST_TIMEOUT,
                GatewayError::new("request timeout"),
            ),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, GatewayError::new("unauthorized")),
            Self::ForbiddenOrigin => (StatusCode::FORBIDDEN, GatewayError::new("forbidden origin")),
            Self::Serialize(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GatewayError::new(INTERNAL_ERROR),
            ),
            Self::Store(MemoryStoreError::InvalidRequest { message }) => {
                (StatusCode::BAD_REQUEST, GatewayError::new(message))
            }
            Self::Store(MemoryStoreError::Unavailable(_)) => (
                StatusCode::SERVICE_UNAVAILABLE,
                GatewayError::new(STORE_UNAVAILABLE),
            ),
            Self::Store(MemoryStoreError::Query { .. }) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GatewayError::new(STORE_QUERY_FAILED),
            ),
            Self::Store(error @ MemoryStoreError::TagTablesMissing) => (
                StatusCode::SERVICE_UNAVAILABLE,
                GatewayError::new(error.to_string()),
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = self.status_and_body();
        (status, Json(body)).into_response()
    }
}
