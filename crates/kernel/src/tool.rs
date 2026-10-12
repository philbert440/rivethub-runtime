use crate::BoxError;
use crate::ToolContext;
use async_trait::async_trait;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("tool is unknown: {0}")]
    Unknown(String),
    #[error("tool arguments are invalid: {0}")]
    InvalidArguments(String),
    #[error("tool execution failed: {message}")]
    Execution {
        message: String,
        #[source]
        source: BoxError,
    },
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> rmcp::model::Tool;

    async fn call(
        &self,
        arguments: Option<rmcp::model::JsonObject>,
        ctx: ToolContext,
    ) -> Result<rmcp::model::CallToolResult, ToolError>;
}
