use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use kernel::memory::MemoryStoreError;
use wire::gateway::GatewayError;
use wire::mesh::MeshParseCode;

const INVALID_JSON: &str = "invalid JSON";
const BODY_MUST_BE_OBJECT: &str = "body must be a JSON object";
const INTERNAL_ERROR: &str = "internal error";
const STORE_UNAVAILABLE: &str = "memory store unavailable";
const STORE_QUERY_FAILED: &str = "memory query failed";

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("body must be a JSON object")]
    BodyMustBeObject,
    #[error(transparent)]
    Transcript(#[from] kernel::transcript::TranscriptReaderError),
    #[error("unknown command: {0}")]
    UnknownCommand(String),
    #[error(transparent)]
    Terminal(#[from] kernel::terminal::TerminalMuxError),
    #[error(transparent)]
    Harness(#[from] kernel::harness::HarnessAdapterError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Task(#[from] tokio::task::JoinError),
    #[error("session is owned by another user")]
    SessionForbidden,
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unprocessable(String),
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
    #[error("internal error")]
    Internal,
    #[error("mesh.json parse failed")]
    MeshParse(MeshParseCode),
    #[error(transparent)]
    Store(#[from] MemoryStoreError),
}

pub fn invalid_json() -> ApiError {
    ApiError::BadRequest(INVALID_JSON.to_owned())
}

pub fn body_must_be_object() -> ApiError {
    ApiError::BodyMustBeObject
}

impl ApiError {
    fn status_and_body(self) -> (StatusCode, GatewayError) {
        match self {
            Self::BodyMustBeObject => (
                StatusCode::BAD_REQUEST,
                GatewayError::new(BODY_MUST_BE_OBJECT),
            ),
            Self::Transcript(error) => (
                StatusCode::SERVICE_UNAVAILABLE,
                GatewayError::new(error.to_string()),
            ),
            Self::UnknownCommand(command) => (
                StatusCode::NOT_FOUND,
                GatewayError::new(format!("unknown command: {command}")),
            ),
            Self::Terminal(error) => (
                StatusCode::SERVICE_UNAVAILABLE,
                GatewayError::new(error.to_string()),
            ),
            Self::Harness(error) => (
                StatusCode::SERVICE_UNAVAILABLE,
                GatewayError::new(error.to_string()),
            ),
            Self::Io(_) | Self::Task(_) | Self::Serialize(_) | Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GatewayError::new(INTERNAL_ERROR),
            ),
            Self::SessionForbidden => (
                StatusCode::FORBIDDEN,
                GatewayError::new("session is owned by another user"),
            ),
            Self::Forbidden(message) => (StatusCode::FORBIDDEN, GatewayError::new(message)),
            Self::Conflict(message) => (StatusCode::CONFLICT, GatewayError::new(message)),
            Self::Unavailable(message) => {
                (StatusCode::SERVICE_UNAVAILABLE, GatewayError::new(message))
            }
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, GatewayError::new(message)),
            Self::Unprocessable(message) => {
                (StatusCode::UNPROCESSABLE_ENTITY, GatewayError::new(message))
            }
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
            Self::MeshParse(code) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GatewayError::new("mesh.json parse failed").code(code.to_string()),
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
