use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use kernel::plugins::PluginTools as _;
use serde::Serialize;
use serde::de::DeserializeOwned;
use wire::plugin::action::{ViewAction, ViewActionRefusal, ViewActionRequest, ViewActionResult};
use wire::plugin::canvas::{CanvasEventRequest, CanvasResponse};
use wire::plugin::host::PluginLogStream;
use wire::plugin::process::PluginToolCall;
use wire::plugin::view::{View, ViewBlock, ViewRequest};
use wire::plugin::{PluginId, ToolName};

use super::{PluginRouteError, host, plugin_id};
use crate::Gateway;
use crate::plugins::PluginHost;
use crate::tenancy::UserId;

const REQUEST_LIMIT: usize = 16 * 1024 * 1024;
const ANSWER_LIMIT: usize = 4 * 1024 * 1024;
const TYPED_TIMEOUT: Duration = Duration::from_secs(30);
const HTTP_FIRST_BYTE: Duration = Duration::from_secs(60);
const SCENE_LIMIT: usize = 5000;
const DETAIL_LIMIT: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
enum Kind {
    Views,
    Actions,
    Canvas,
    Tools,
    Panel,
    Http,
}

pub(super) async fn forward(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path((id, kind, rest)): Path<(String, String, String)>,
    request: Request,
) -> Response {
    match route(&gateway, &user, id, &kind, &rest, request).await {
        Ok(response) => response,
        Err(error) => error.into_response(),
    }
}

async fn route(
    gateway: &Gateway,
    user: &UserId,
    id: String,
    kind: &str,
    rest: &str,
    request: Request,
) -> Result<Response, PluginRouteError> {
    let host = host(gateway)?;
    let plugin = plugin_id(id)?;
    let kind: Kind = kind
        .parse()
        .map_err(|_| PluginRouteError::NotFound(format!("no plug-in route {kind}")))?;
    if kind == Kind::Tools {
        return tool(host, &plugin, rest, request).await;
    }
    let client = host.proxy_client(&plugin)?;
    let prefix = format!("/api/plugins/{}", plugin.as_ref());
    let suffix = request
        .uri()
        .path()
        .strip_prefix(prefix.as_str())
        .ok_or_else(|| PluginRouteError::NotFound("no plug-in route".into()))?
        .to_owned();
    let query = request
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let target = Target {
        host,
        plugin: &plugin,
        client: &client,
        url: format!("http://plugin{suffix}{query}"),
        headers: forwarded(gateway, user),
    };
    match kind {
        Kind::Views => {
            let answer = typed::<ViewRequest, View>(&target, request, |view| {
                let items = canvas_items(&view.blocks);
                if items > SCENE_LIMIT {
                    Err(format!(
                        "blocks: {items} canvas items, more than {SCENE_LIMIT}"
                    ))
                } else {
                    Ok(())
                }
            })
            .await?;
            Ok(answer.into_response())
        }
        Kind::Actions => actions(&target, request).await,
        Kind::Canvas => {
            let answer = typed::<CanvasEventRequest, CanvasResponse>(&target, request, |answer| {
                no_call(&answer.actions)
            })
            .await?;
            Ok(answer.into_response())
        }
        Kind::Panel | Kind::Http => passthrough(&target, request).await,
        Kind::Tools => Err(PluginRouteError::NotFound("no plug-in route".into())),
    }
}

struct Target<'a> {
    host: &'a PluginHost,
    plugin: &'a PluginId,
    client: &'a reqwest::Client,
    url: String,
    headers: HeaderMap,
}

impl Target<'_> {
    fn refused(&self, detail: String) -> PluginRouteError {
        self.host.log(
            self.plugin,
            PluginLogStream::Host,
            format!("refused the {} answer: {detail}", self.url),
        );
        PluginRouteError::Invalid(detail)
    }
}

fn forwarded(gateway: &Gateway, user: &UserId) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let pairs = [
        ("x-rivethub-user", user.as_str().to_owned()),
        ("x-rivethub-node", gateway.settings().node.clone()),
        ("x-rivethub-request-id", uuid::Uuid::new_v4().to_string()),
    ];
    for (name, value) in pairs {
        match HeaderValue::from_str(&value) {
            Ok(value) => {
                headers.insert(HeaderName::from_static(name), value);
            }
            Err(error) => tracing::debug!(%error, name, "plug-in proxy header left out"),
        }
    }
    headers
}

async fn read_request<T: DeserializeOwned>(request: Request) -> Result<T, PluginRouteError> {
    if request.method() != Method::POST {
        return Err(PluginRouteError::BadRequest(
            "this plug-in route takes POST".into(),
        ));
    }
    let bytes = axum::body::to_bytes(request.into_body(), REQUEST_LIMIT)
        .await
        .map_err(|error| PluginRouteError::TooLarge(error.to_string()))?;
    crate::plugins::parse(&bytes).map_err(PluginRouteError::BadRequest)
}

async fn send<T: Serialize>(
    target: &Target<'_>,
    body: &T,
) -> Result<(reqwest::StatusCode, Bytes), PluginRouteError> {
    let response = target
        .client
        .post(&target.url)
        .headers(target.headers.clone())
        .timeout(TYPED_TIMEOUT)
        .json(body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                PluginRouteError::Timeout(format!(
                    "plug-in {} did not answer in time",
                    target.plugin.as_ref()
                ))
            } else {
                PluginRouteError::Failed(error.to_string())
            }
        })?;
    let status = response.status();
    let bytes = capped(response).await?;
    Ok((status, bytes))
}

async fn capped(mut response: reqwest::Response) -> Result<Bytes, PluginRouteError> {
    let mut collected = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| PluginRouteError::Failed(error.to_string()))?
    {
        collected.extend_from_slice(&chunk);
        if collected.len() > ANSWER_LIMIT {
            return Err(PluginRouteError::Invalid(format!(
                "the answer is larger than {ANSWER_LIMIT} bytes"
            )));
        }
    }
    Ok(Bytes::from(collected))
}

fn unexpected(status: reqwest::StatusCode, bytes: &Bytes) -> PluginRouteError {
    let shown = bytes
        .get(..bytes.len().min(DETAIL_LIMIT))
        .unwrap_or_default();
    PluginRouteError::Failed(format!(
        "the plug-in answered {status}: {}",
        String::from_utf8_lossy(shown)
    ))
}

async fn typed<Asked, Answer>(
    target: &Target<'_>,
    request: Request,
    check: impl FnOnce(&Answer) -> Result<(), String>,
) -> Result<Json<Answer>, PluginRouteError>
where
    Asked: DeserializeOwned + Serialize,
    Answer: DeserializeOwned + Serialize,
{
    let asked: Asked = read_request(request).await?;
    let (status, bytes) = send(target, &asked).await?;
    if status != reqwest::StatusCode::OK {
        return Err(unexpected(status, &bytes));
    }
    let answer: Answer = crate::plugins::parse(&bytes).map_err(|detail| target.refused(detail))?;
    check(&answer).map_err(|detail| target.refused(detail))?;
    Ok(Json(answer))
}

async fn actions(target: &Target<'_>, request: Request) -> Result<Response, PluginRouteError> {
    let asked: ViewActionRequest = read_request(request).await?;
    let (status, bytes) = send(target, &asked).await?;
    match status {
        reqwest::StatusCode::OK => {
            let answer: ViewActionResult =
                crate::plugins::parse(&bytes).map_err(|detail| target.refused(detail))?;
            no_call(&answer.actions).map_err(|detail| target.refused(detail))?;
            Ok(Json(answer).into_response())
        }
        reqwest::StatusCode::UNPROCESSABLE_ENTITY => {
            let refusal: ViewActionRefusal =
                crate::plugins::parse(&bytes).map_err(|detail| target.refused(detail))?;
            Ok((StatusCode::UNPROCESSABLE_ENTITY, Json(refusal)).into_response())
        }
        other => Err(unexpected(other, &bytes)),
    }
}

async fn tool(
    host: &PluginHost,
    plugin: &PluginId,
    rest: &str,
    request: Request,
) -> Result<Response, PluginRouteError> {
    let tool = ToolName::try_from(rest.to_owned())
        .map_err(|error| PluginRouteError::NotFound(error.to_string()))?;
    let call: PluginToolCall = read_request(request).await?;
    match host.call(plugin, &tool, call, None).await {
        Ok(result) => Ok(Json(result).into_response()),
        Err(error) => Ok(crate::plugins::tool_error(&error)),
    }
}

fn no_call(actions: &[ViewAction]) -> Result<(), String> {
    match actions
        .iter()
        .position(|action| matches!(action, ViewAction::Call { .. }))
    {
        Some(index) => Err(format!(
            "actions[{index}]: a call cannot answer with another call"
        )),
        None => Ok(()),
    }
}

fn canvas_items(blocks: &[ViewBlock]) -> usize {
    blocks
        .iter()
        .map(|block| match block {
            ViewBlock::Canvas { scene, .. } => scene.items.len(),
            ViewBlock::Section { blocks, .. } => canvas_items(blocks),
            ViewBlock::Split {
                primary, secondary, ..
            } => canvas_items(primary) + canvas_items(secondary),
            ViewBlock::Tabs { tabs, .. } => tabs.iter().map(|tab| canvas_items(&tab.blocks)).sum(),
            ViewBlock::Text { .. }
            | ViewBlock::Markdown { .. }
            | ViewBlock::Code { .. }
            | ViewBlock::Image { .. }
            | ViewBlock::Properties { .. }
            | ViewBlock::Progress { .. }
            | ViewBlock::Notice { .. }
            | ViewBlock::List { .. }
            | ViewBlock::Table { .. }
            | ViewBlock::Breadcrumbs { .. }
            | ViewBlock::Actions { .. }
            | ViewBlock::Form { .. }
            | ViewBlock::WebPanel { .. } => 0,
        })
        .sum()
}

const HOP_BY_HOP: [HeaderName; 8] = [
    header::CONNECTION,
    header::PROXY_AUTHENTICATE,
    header::PROXY_AUTHORIZATION,
    header::TE,
    header::TRAILER,
    header::TRANSFER_ENCODING,
    header::UPGRADE,
    header::HOST,
];

fn kept(name: &HeaderName) -> bool {
    !HOP_BY_HOP.contains(name)
        && name != header::AUTHORIZATION
        && name != header::COOKIE
        && name != header::SET_COOKIE
        && name.as_str() != "keep-alive"
}

async fn passthrough(target: &Target<'_>, request: Request) -> Result<Response, PluginRouteError> {
    let (parts, body) = request.into_parts();
    let mut headers: HeaderMap = parts
        .headers
        .iter()
        .filter(|(name, _)| kept(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    headers.extend(target.headers.clone());
    let pending = target
        .client
        .request(parts.method, &target.url)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send();
    let response = tokio::time::timeout(HTTP_FIRST_BYTE, pending)
        .await
        .map_err(|_| {
            PluginRouteError::Timeout(format!(
                "plug-in {} did not answer in time",
                target.plugin.as_ref()
            ))
        })?
        .map_err(|error| PluginRouteError::Failed(error.to_string()))?;
    let mut answer = Response::builder().status(response.status());
    for (name, value) in response.headers() {
        if kept(name) {
            answer = answer.header(name, value);
        }
    }
    answer
        .body(axum::body::Body::from_stream(response.bytes_stream()))
        .map_err(|error| PluginRouteError::Failed(error.to_string()))
}
