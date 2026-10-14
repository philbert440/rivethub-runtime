use crate::Error;
use async_trait::async_trait;
use kernel::{
    ToolContext,
    tool::{Tool, ToolError},
};
use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ClientConfig},
    service::{QuitReason, RoleClient, RunningService},
    transport::IntoTransport,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct Client {
    service: RunningService<RoleClient, ClientConfig>,
    shutdown: CancellationToken,
}

impl Client {
    pub async fn connect<T, E, A>(transport: T, shutdown: CancellationToken) -> Result<Self, Error>
    where
        T: IntoTransport<RoleClient, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let service = ClientConfig::default()
            .serve_with_ct(transport, shutdown.child_token())
            .await
            .map_err(|error| Error::Initialize(Box::new(error)))?;
        Ok(Self { service, shutdown })
    }

    pub async fn tools(&self) -> Result<Vec<Arc<dyn Tool>>, Error> {
        let definitions = tokio::select! {
            () = self.shutdown.cancelled() => return Err(Error::Cancelled),
            definitions = self.service.list_all_tools() => definitions?,
        };
        Ok(definitions
            .into_iter()
            .map(|definition| {
                Arc::new(RemoteTool {
                    definition,
                    peer: self.service.peer().clone(),
                    shutdown: self.shutdown.clone(),
                }) as Arc<dyn Tool>
            })
            .collect())
    }

    pub async fn close(self) -> Result<(), Error> {
        match self.service.cancel().await? {
            QuitReason::JoinError(error) => Err(Error::Task(error)),
            QuitReason::Closed | QuitReason::Cancelled => Ok(()),
            other => Err(Error::Invalid(format!(
                "unexpected MCP shutdown: {other:?}"
            ))),
        }
    }
}

#[derive(Debug)]
struct RemoteTool {
    definition: rmcp::model::Tool,
    peer: rmcp::Peer<RoleClient>,
    shutdown: CancellationToken,
}

#[async_trait]
impl Tool for RemoteTool {
    fn definition(&self) -> rmcp::model::Tool {
        self.definition.clone()
    }

    async fn call(
        &self,
        arguments: Option<rmcp::model::JsonObject>,
        _ctx: ToolContext,
    ) -> Result<CallToolResult, ToolError> {
        let mut request = CallToolRequestParams::new(self.definition.name.clone());
        request.arguments = arguments;
        let result = tokio::select! {
            () = self.shutdown.cancelled() => Err(Error::Cancelled),
            result = self.peer.call_tool(request) => result.map_err(Error::from),
        };
        result.map_err(|error| ToolError::Execution {
            message: error.to_string(),
            source: Box::new(error),
        })
    }
}
