pub mod catalog;
pub mod client;
pub mod error;
pub mod memory;
pub mod tags;
pub mod transport;
pub mod wiki;

pub use error::Error;
use kernel::memory::MemoryStore;
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData, ServerHandler};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct Server {
    memory: Arc<dyn MemoryStore>,
    tools: Arc<Vec<Tool>>,
    memory_write: bool,
    shutdown: CancellationToken,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "MemoryStore is a kernel trait object without Debug; the formatter deliberately omits it and the tool catalog"
    )
)]
impl std::fmt::Debug for Server {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("memory_write", &self.memory_write)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, strum::EnumString)]
enum Operation {
    #[strum(serialize = "memory_search")]
    Search,
    #[strum(serialize = "memory_browse")]
    Browse,
    #[strum(serialize = "memory_stats")]
    Stats,
    #[strum(serialize = "memory_get_full")]
    GetFull,
    #[strum(serialize = "memory_tags")]
    Tags,
    #[strum(serialize = "wiki_search")]
    WikiSearch,
    #[strum(serialize = "wiki_read")]
    WikiRead,
}

impl Server {
    pub fn new(
        memory: Arc<dyn MemoryStore>,
        memory_write: bool,
        shutdown: CancellationToken,
    ) -> Result<Self, Error> {
        let mut tools = catalog::definitions()?;
        if memory_write {
            let tags = tools
                .iter_mut()
                .find(|tool| tool.name == "memory_tags")
                .ok_or_else(|| Error::Invalid("memory_tags is missing from catalog".into()))?;
            tags.description = Some("Read and manage memory tags. Writes are enabled. taxonomy_upsert fully replaces aliases, parent_value, display and reason; aliases is required and omitted parent_value clears the parent.".into());
            tags.annotations = Some(rmcp::model::ToolAnnotations::from_raw(
                None,
                Some(false),
                Some(true),
                Some(false),
                Some(false),
            ));
        }
        Ok(Self {
            memory,
            tools: Arc::new(tools),
            memory_write,
            shutdown,
        })
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }

    pub async fn call(&self, request: CallToolRequestParams) -> Result<CallToolResult, ErrorData> {
        let operation: Operation = request.name.parse().map_err(|_| {
            ErrorData::invalid_params(format!("Tool {} not found", request.name), None)
        })?;
        let arguments = match request.arguments {
            Some(arguments) => arguments,
            None => rmcp::model::JsonObject::new(),
        };
        let result = tokio::select! {
            biased;
            () = self.shutdown.cancelled() => Err(Error::Cancelled),
            result = self.execute(operation, arguments) => result,
        };
        match result {
            Ok(result) => Ok(result),
            Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(
                error.to_string(),
            )])),
        }
    }

    async fn execute(
        &self,
        operation: Operation,
        arguments: rmcp::model::JsonObject,
    ) -> Result<CallToolResult, Error> {
        match operation {
            Operation::Search => memory::search(self.memory.as_ref(), decode(arguments)?).await,
            Operation::Browse => memory::browse(self.memory.as_ref(), decode(arguments)?).await,
            Operation::Stats => memory::stats(self.memory.as_ref(), decode(arguments)?).await,
            Operation::GetFull => memory::get_full(self.memory.as_ref(), decode(arguments)?).await,
            Operation::Tags => {
                tags::call(self.memory.as_ref(), self.memory_write, decode(arguments)?).await
            }
            Operation::WikiSearch => wiki::search(self.memory.as_ref(), decode(arguments)?).await,
            Operation::WikiRead => wiki::read(self.memory.as_ref(), decode(arguments)?).await,
        }
    }
}

impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(Implementation::new(
            "rivethub-mcp-server",
            env!("CARGO_PKG_VERSION"),
        ))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send {
        std::future::ready(Ok(ListToolsResult {
            tools: self.tools.as_ref().clone(),
            ..ListToolsResult::default()
        }))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, ErrorData> {
        tokio::select! {
            () = context.ct.cancelled() => Err(ErrorData::internal_error("request cancelled", None)),
            result = self.call(request) => result.map(Into::into),
        }
    }
}

fn decode<T: DeserializeOwned>(arguments: rmcp::model::JsonObject) -> Result<T, Error> {
    Ok(serde_json::from_value(serde_json::to_value(arguments)?)?)
}

pub(crate) fn text(value: String) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text(value)]);
    result.is_error = None;
    result
}

pub(crate) fn json<T: serde::Serialize>(value: &T) -> Result<CallToolResult, Error> {
    Ok(text(serde_json::to_string(value)?))
}
