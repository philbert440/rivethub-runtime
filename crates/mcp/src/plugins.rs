use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use kernel::plugins::{PluginToolError, PluginTools};
use rmcp::model::{CallToolResult, ContentBlock, Tool, ToolAnnotations};
use wire::plugin::host::PluginToolList;
use wire::plugin::manifest::PluginToolHint;
use wire::plugin::process::{PluginToolCall, PluginToolResult};
use wire::plugin::{PluginId, ToolName};

const LONG_POLL: Duration = Duration::from_secs(90);
const CALL_TIMEOUT: Duration = Duration::from_secs(130);

pub(crate) fn definitions(list: &PluginToolList) -> Vec<Tool> {
    list.tools
        .iter()
        .map(|entry| {
            let tool = &entry.tool;
            let hinted = |hint| Some(tool.hints.contains(&hint));
            let mut definition = Tool::new(
                kernel::plugins::tool_name(&entry.plugin, &tool.name),
                tool.description.clone(),
                Arc::new(tool.input_schema.as_map().clone()),
            );
            definition.title.clone_from(&tool.title);
            definition.annotations = Some(ToolAnnotations::from_raw(
                tool.title.clone(),
                hinted(PluginToolHint::ReadOnly),
                hinted(PluginToolHint::Destructive),
                hinted(PluginToolHint::Idempotent),
                hinted(PluginToolHint::OpenWorld),
            ));
            definition
        })
        .collect()
}

const TOOL_USE_META: &str = "claudecode/toolUseId";
const TOOL_USE_HEADER: &str = "x-rivethub-tool-use";

pub(crate) fn tool_use(meta: &rmcp::model::RequestMetaObject) -> Option<String> {
    meta.get(TOOL_USE_META)
        .and_then(|value| value.as_str())
        .map(str::to_owned)
}

pub(crate) async fn call(
    port: &dyn PluginTools,
    name: &str,
    arguments: rmcp::model::JsonObject,
    tool_use: Option<&str>,
) -> CallToolResult {
    let answered = match kernel::plugins::split_tool_name(name) {
        Ok((plugin, tool)) => match wire::JsonObject::from_serialize(&arguments) {
            Ok(arguments) => {
                port.call(
                    &plugin,
                    &tool,
                    PluginToolCall {
                        arguments,
                        session_id: None,
                    },
                    tool_use,
                )
                .await
            }
            Err(error) => Err(PluginToolError::Failed {
                plugin: plugin.as_ref().to_owned(),
                tool: tool.as_ref().to_owned(),
                detail: error.to_string(),
            }),
        },
        Err(error) => Err(error),
    };
    match answered {
        Ok(result) => {
            let mut answer = CallToolResult::success(vec![ContentBlock::text(result.text)]);
            answer.is_error = Some(result.is_error);
            answer
        }
        Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    }
}

#[derive(Debug, Clone)]
pub struct SocketTools {
    http: reqwest::Client,
}

impl SocketTools {
    pub fn new(socket: PathBuf) -> Result<Self, crate::Error> {
        let http = reqwest::Client::builder()
            .unix_socket(socket)
            .build()
            .map_err(|error| crate::Error::Invalid(format!("plug-in tools socket: {error}")))?;
        Ok(Self { http })
    }

    async fn list(
        &self,
        after: Option<u64>,
        wait: Duration,
    ) -> Result<PluginToolList, PluginToolError> {
        let url = match after {
            Some(seen) => format!("http://plugins/tools?after={seen}"),
            None => "http://plugins/tools".to_owned(),
        };
        let response = self
            .http
            .get(url)
            .timeout(wait)
            .send()
            .await
            .map_err(|error| PluginToolError::Unreachable(error.to_string()))?;
        if !response.status().is_success() {
            return Err(PluginToolError::Unreachable(format!(
                "the tools socket answered {}",
                response.status()
            )));
        }
        response
            .json::<PluginToolList>()
            .await
            .map_err(|error| PluginToolError::Unreachable(error.to_string()))
    }
}

#[async_trait]
impl PluginTools for SocketTools {
    async fn tools(&self) -> Result<PluginToolList, PluginToolError> {
        self.list(None, CALL_TIMEOUT).await
    }

    async fn call(
        &self,
        plugin: &PluginId,
        tool: &ToolName,
        call: PluginToolCall,
        tool_use: Option<&str>,
    ) -> Result<PluginToolResult, PluginToolError> {
        let name = kernel::plugins::tool_name(plugin, tool);
        let request = self
            .http
            .post(format!("http://plugins/tools/{name}"))
            .timeout(CALL_TIMEOUT)
            .json(&call);
        let request = match tool_use {
            Some(id) => request.header(TOOL_USE_HEADER, id),
            None => request,
        };
        let response = request
            .send()
            .await
            .map_err(|error| PluginToolError::Unreachable(error.to_string()))?;
        if response.status().is_success() {
            return response
                .json::<PluginToolResult>()
                .await
                .map_err(|error| PluginToolError::Unreachable(error.to_string()));
        }
        let status = response.status();
        let refused = response
            .json::<wire::gateway::GatewayError>()
            .await
            .map_err(|error| PluginToolError::Unreachable(error.to_string()))?;
        Err(PluginToolError::Failed {
            plugin: plugin.as_ref().to_owned(),
            tool: tool.as_ref().to_owned(),
            detail: format!("{status}: {}", refused.error),
        })
    }

    async fn changed(&self, seen: u64) -> Result<u64, PluginToolError> {
        loop {
            let list = self.list(Some(seen), LONG_POLL).await?;
            if list.version != seen {
                return Ok(list.version);
            }
        }
    }
}
