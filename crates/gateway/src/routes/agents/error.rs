use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use kernel::agent_presets::AgentPresetStoreError;
use wire::gateway::GatewayError;
use wire::harness::HarnessId;

#[derive(Debug, thiserror::Error)]
pub(crate) enum AgentError {
    #[error("{0}")]
    BadRequest(&'static str),
    #[error("{0}")]
    BadRequestOwned(String),
    #[error("agent not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("agent \"{name}\" is hosted on {node}")]
    Hosted { name: String, node: String },
    #[error("harness \"{harness}\" is not allowed on this node (den.allowed_harnesses)")]
    HarnessDenied { harness: HarnessId },
    #[error("could not create agent directory: {0}")]
    Directory(#[source] std::io::Error),
    #[error("agent registry unavailable")]
    Unavailable,
}

impl AgentError {
    pub(crate) fn name_taken(name: &str) -> Self {
        Self::Conflict(format!("an agent named \"{name}\" already exists"))
    }

    pub(crate) fn from_store(error: AgentPresetStoreError) -> Self {
        match error {
            AgentPresetStoreError::NameTaken { name } => Self::name_taken(&name),
            AgentPresetStoreError::Unavailable(source) => {
                tracing::warn!(error = %source, "agent preset store failed");
                Self::Unavailable
            }
        }
    }

    fn status_and_body(&self) -> (StatusCode, GatewayError) {
        match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, GatewayError::new(*message)),
            Self::BadRequestOwned(message) => {
                (StatusCode::BAD_REQUEST, GatewayError::new(message.clone()))
            }
            Self::NotFound => (StatusCode::NOT_FOUND, GatewayError::new(self.to_string())),
            Self::Conflict(message) => (StatusCode::CONFLICT, GatewayError::new(message.clone())),
            Self::Hosted { name, node } => {
                let mut body = GatewayError::new(format!("agent \"{name}\" is hosted on {node}"));
                body.node = Some(node.clone());
                (StatusCode::CONFLICT, body)
            }
            Self::HarnessDenied { harness } => {
                let mut body = GatewayError::new(self.to_string()).code("harness_not_allowed");
                body.harness_id = Some(*harness);
                (StatusCode::FORBIDDEN, body)
            }
            Self::Directory(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                GatewayError::new(self.to_string()),
            ),
            Self::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                GatewayError::new(self.to_string()),
            ),
        }
    }
}

impl IntoResponse for AgentError {
    fn into_response(self) -> Response {
        let (status, body) = self.status_and_body();
        (status, Json(body)).into_response()
    }
}
