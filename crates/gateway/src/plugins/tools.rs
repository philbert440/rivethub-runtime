use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kernel::plugins::{PluginToolError, PluginTools};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use subtle::ConstantTimeEq as _;
use tokio::net::UnixListener;
use tokio_util::sync::CancellationToken;
use wire::gateway::GatewayError;
use wire::plugin::host::{PluginState, PluginToolEntry, PluginToolList};
use wire::plugin::process::{PluginToolCall, PluginToolResult};
use wire::plugin::{PluginId, ToolName};

use super::PluginHost;
use crate::extract::Body;

const TOOL_TIMEOUT: Duration = Duration::from_secs(120);
const LIST_WAIT: Duration = Duration::from_secs(60);
const TOOL_USE_HEADER: &str = "x-rivethub-tool-use";
const CALLER_WAIT: Duration = Duration::from_secs(1);
const CALLER_POLL: Duration = Duration::from_millis(50);

#[async_trait]
impl PluginTools for PluginHost {
    async fn tools(&self) -> Result<PluginToolList, PluginToolError> {
        let version = *self.tools.borrow();
        let running: Vec<PluginToolEntry> = self
            .entries
            .lock()
            .iter()
            .filter(|(_, entry)| entry.live.state == PluginState::Running)
            .flat_map(|(id, entry)| {
                entry
                    .stored
                    .manifest
                    .tools
                    .iter()
                    .map(|tool| PluginToolEntry {
                        plugin: id.clone(),
                        tool: tool.clone(),
                    })
            })
            .collect();
        let tools = self.creator.iter().cloned().chain(running).collect();
        Ok(PluginToolList { version, tools })
    }

    async fn call(
        &self,
        plugin: &PluginId,
        tool: &ToolName,
        mut call: PluginToolCall,
        tool_use: Option<&str>,
    ) -> Result<PluginToolResult, PluginToolError> {
        if plugin.as_ref() == super::creator::NAMESPACE {
            return Ok(self.create(tool, &call.arguments, tool_use).await);
        }
        let declared = self
            .peek(plugin, |entry| {
                entry
                    .stored
                    .manifest
                    .tools
                    .iter()
                    .any(|known| &known.name == tool)
            })
            .is_some_and(|known| known);
        if !declared {
            return Err(PluginToolError::UnknownTool(
                plugin.as_ref().to_owned(),
                tool.as_ref().to_owned(),
            ));
        }
        let client = self
            .running_client(plugin)
            .ok_or_else(|| PluginToolError::NotRunning(plugin.as_ref().to_owned()))?;
        if call.session_id.is_none()
            && let Some(tool_use) = tool_use
        {
            call.session_id = self.calling_session(tool_use).await;
        }
        let failed = |detail: String| PluginToolError::Failed {
            plugin: plugin.as_ref().to_owned(),
            tool: tool.as_ref().to_owned(),
            detail,
        };
        let response = client
            .post(super::plugin_url(&format!("tools/{}", tool.as_ref())))
            .timeout(TOOL_TIMEOUT)
            .json(&call)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    PluginToolError::Timeout {
                        plugin: plugin.as_ref().to_owned(),
                        tool: tool.as_ref().to_owned(),
                    }
                } else {
                    failed(error.to_string())
                }
            })?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| failed(error.to_string()))?;
        if status != reqwest::StatusCode::OK {
            return Err(failed(format!("answered {status}")));
        }
        super::parse::<PluginToolResult>(&bytes).map_err(|detail| {
            self.log(
                plugin,
                wire::plugin::host::PluginLogStream::Host,
                format!("refused the tools/{} answer: {detail}", tool.as_ref()),
            );
            PluginToolError::Invalid {
                plugin: plugin.as_ref().to_owned(),
                tool: tool.as_ref().to_owned(),
                detail,
            }
        })
    }

    async fn changed(&self, seen: u64) -> Result<u64, PluginToolError> {
        let mut versions = self.tools.subscribe();
        let version = versions
            .wait_for(|version| *version != seen)
            .await
            .map_err(|error| PluginToolError::Unreachable(error.to_string()))?;
        Ok(*version)
    }
}

impl PluginHost {
    pub(super) async fn calling_session(
        &self,
        tool_use: &str,
    ) -> Option<wire::session_id::SessionId> {
        let terminal = self.terminal.get().and_then(std::sync::Weak::upgrade)?;
        let deadline = tokio::time::Instant::now() + CALLER_WAIT;
        loop {
            if let Some(session) = terminal.running_tool(tool_use) {
                return Some(session);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(CALLER_POLL).await;
        }
    }
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    after: Option<u64>,
}

pub(super) async fn issue(path: &std::path::Path) -> Result<[u8; 32], super::HostError> {
    let mut secret = [0_u8; 32];
    getrandom::fill(&mut secret).map_err(|error| std::io::Error::other(error.to_string()))?;
    let token = hex::encode(secret);
    super::registry::write_private(path, token.as_bytes()).await?;
    Ok(Sha256::digest(token.as_bytes()).into())
}

async fn holder(
    State(host): State<Arc<PluginHost>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let presented = match request
        .headers()
        .get(header::AUTHORIZATION)
        .map(HeaderValue::to_str)
    {
        Some(Ok(text)) => text.strip_prefix("Bearer "),
        Some(Err(_)) | None => None,
    };
    let held = match (presented, host.tools_key.get()) {
        (Some(token), Some(key)) => {
            let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
            bool::from(digest.ct_eq(key))
        }
        (Some(_) | None, _) => false,
    };
    if held {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            Json(GatewayError::new("the tools socket needs its token").code("unauthorized")),
        )
            .into_response()
    }
}

pub(super) async fn serve(host: Arc<PluginHost>, listener: UnixListener, stop: CancellationToken) {
    let router = Router::new()
        .route("/tools", get(list))
        .route("/tools/{name}", post(call))
        .layer(middleware::from_fn_with_state(Arc::clone(&host), holder))
        .with_state(host);
    if let Err(error) = axum::serve(listener, router)
        .with_graceful_shutdown(stop.cancelled_owned())
        .await
    {
        tracing::error!(%error, "plug-in tools socket stopped");
    }
}

async fn list(State(host): State<Arc<PluginHost>>, Query(query): Query<ListQuery>) -> Response {
    if let Some(seen) = query.after
        && *host.tools.borrow() == seen
    {
        let waited =
            tokio::time::timeout(LIST_WAIT, PluginTools::changed(host.as_ref(), seen)).await;
        if let Err(elapsed) = waited {
            tracing::trace!(%elapsed, "plug-in tool list unchanged");
        }
    }
    match PluginTools::tools(host.as_ref()).await {
        Ok(list) => Json(list).into_response(),
        Err(error) => tool_error(&error),
    }
}

async fn call(
    State(host): State<Arc<PluginHost>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Body(request): Body<PluginToolCall>,
) -> Response {
    #[cfg_attr(
        dylint_lib = "rr_lints",
        expect(
            fallback,
            reason = "PATTERNS FALLBACK: a non-UTF-8 header is no header, so the call names no tool use"
        )
    )]
    let tool_use = headers
        .get(TOOL_USE_HEADER)
        .and_then(|value| value.to_str().ok());
    let answered = match kernel::plugins::split_tool_name(&name) {
        Ok((plugin, tool)) => {
            PluginTools::call(host.as_ref(), &plugin, &tool, request, tool_use).await
        }
        Err(error) => Err(error),
    };
    match answered {
        Ok(result) => Json(result).into_response(),
        Err(error) => tool_error(&error),
    }
}

pub(crate) fn tool_error(error: &PluginToolError) -> Response {
    let (status, code) = match error {
        PluginToolError::NotPluginTool(_)
        | PluginToolError::Name(_)
        | PluginToolError::UnknownTool(..) => (StatusCode::NOT_FOUND, "not_found"),
        PluginToolError::NotRunning(_) | PluginToolError::Unreachable(_) => {
            (StatusCode::SERVICE_UNAVAILABLE, "plugin_unavailable")
        }
        PluginToolError::Timeout { .. } => (StatusCode::GATEWAY_TIMEOUT, "plugin_timeout"),
        PluginToolError::Invalid { .. } => (StatusCode::BAD_GATEWAY, "plugin_invalid_response"),
        PluginToolError::Failed { .. } => (StatusCode::BAD_GATEWAY, "plugin_error"),
    };
    (
        status,
        Json(GatewayError::new(error.to_string()).code(code)),
    )
        .into_response()
}
