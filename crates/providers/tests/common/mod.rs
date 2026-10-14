use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use axum::body::Body;
use axum::body::Bytes;
use axum::response::IntoResponse;
use futures::stream::unfold;
use kernel::model::ModelChunk;
use kernel::model::ModelProvider;
use kernel::model::ModelRequest;
use kernel::model::ModelStreamItem;
use parking_lot::Mutex;
use providers::OpenAiCompatible;
use serde::Deserialize;
use serde::Serialize;
use testkit::Canned;
use testkit::CapturedRequest;
use testkit::Json;
use testkit::ProviderTurn;
use testkit::provider::ProviderScenario;
use wire::message::ContentPart;
use wire::message::Message;
use wire::message::MessageContent;
use wire::message::ToolCall;
use wire::stream::LlmUsage;
use wire::stream::ThinkingLevel;

pub(crate) fn demand<T, E: std::fmt::Display>(value: Result<T, E>, what: &str) -> T {
    match value {
        Ok(value) => value,
        Err(error) => halt(&format!("{what}: {error}")),
    }
}

pub(crate) fn demand_some<T>(value: Option<T>, what: &str) -> T {
    match value {
        Some(value) => value,
        None => halt(what),
    }
}

#[expect(
    clippy::panic,
    reason = "the harness panics when a fixture or a local stub fails to set up"
)]
pub(crate) fn halt(what: &str) -> ! {
    panic!("{what}");
}

pub(crate) type Recorder = Arc<Mutex<Vec<CapturedCall>>>;

#[derive(Debug, Clone)]
pub(crate) struct CapturedCall {
    pub(crate) method: String,
    pub(crate) target: String,
    pub(crate) authorization: Option<String>,
    pub(crate) body: String,
}

pub(crate) struct ScenarioServer {
    pub(crate) addr: SocketAddr,
    pub(crate) recorder: Recorder,
    _stub: testkit::Stub,
}

pub(crate) async fn scenario_server(canned: Vec<Canned>) -> ScenarioServer {
    let recorder: Recorder = Arc::default();
    let queue = Arc::new(Mutex::new(VecDeque::from(canned)));
    let routes = axum::Router::new()
        .fallback(canned_handler)
        .with_state((recorder.clone(), queue));
    let stub = demand(testkit::stub_server(routes).await, "stub server binds");
    ScenarioServer {
        addr: stub.addr,
        recorder,
        _stub: stub,
    }
}

async fn canned_handler(
    axum::extract::State((recorder, queue)): axum::extract::State<(
        Recorder,
        Arc<Mutex<VecDeque<Canned>>>,
    )>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let call = capture(request).await;
    recorder.lock().push(call);
    let canned = queue.lock().pop_front();
    let Some(canned) = canned else {
        return axum::http::StatusCode::GONE.into_response();
    };
    let status = demand(
        axum::http::StatusCode::from_u16(canned.status),
        "recorded status",
    );
    (status, canned.body).into_response()
}

pub(crate) struct RepeatingServer {
    pub(crate) addr: SocketAddr,
    pub(crate) recorder: Recorder,
    _stub: testkit::Stub,
}

pub(crate) async fn repeating_server(body: String) -> RepeatingServer {
    let recorder: Recorder = Arc::default();
    let routes = axum::Router::new()
        .fallback(streaming_handler)
        .with_state((recorder.clone(), Arc::new(body)));
    let stub = demand(testkit::stub_server(routes).await, "stub server binds");
    RepeatingServer {
        addr: stub.addr,
        recorder,
        _stub: stub,
    }
}

pub(crate) struct SlowServer {
    pub(crate) addr: SocketAddr,
    pub(crate) recorder: Recorder,
    pub(crate) disconnect: Disconnect,
    _stub: testkit::Stub,
}

pub(crate) async fn slow_server(first: String) -> SlowServer {
    let recorder: Recorder = Arc::default();
    let disconnect = Disconnect::default();
    let routes = axum::Router::new().fallback(slow_handler).with_state((
        recorder.clone(),
        Arc::new(first),
        disconnect.clone(),
    ));
    let stub = demand(testkit::stub_server(routes).await, "stub server binds");
    SlowServer {
        addr: stub.addr,
        recorder,
        disconnect,
        _stub: stub,
    }
}

pub(crate) struct FixedStatusServer {
    pub(crate) addr: SocketAddr,
    pub(crate) recorder: Recorder,
    _stub: testkit::Stub,
}

pub(crate) async fn fixed_status_server(status: u16, body: String) -> FixedStatusServer {
    let recorder: Recorder = Arc::default();
    let routes = axum::Router::new()
        .fallback(fixed_handler)
        .with_state((recorder.clone(), (status, body)));
    let stub = demand(testkit::stub_server(routes).await, "stub server binds");
    FixedStatusServer {
        addr: stub.addr,
        recorder,
        _stub: stub,
    }
}

#[derive(Clone, Default)]
pub(crate) struct Disconnect {
    flag: Arc<Mutex<bool>>,
    done: Arc<Mutex<bool>>,
}

impl Disconnect {
    pub(crate) fn is_disconnected(&self) -> bool {
        *self.flag.lock()
    }

    pub(crate) fn wrote_done(&self) -> bool {
        *self.done.lock()
    }
}

struct DisconnectGuard(Disconnect);

impl Drop for DisconnectGuard {
    fn drop(&mut self) {
        *self.0.flag.lock() = true;
    }
}

async fn slow_handler(
    axum::extract::State((recorder, body, disconnect)): axum::extract::State<(
        Recorder,
        Arc<String>,
        Disconnect,
    )>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let call = capture(request).await;
    recorder.lock().push(call);
    let event = body.as_bytes().to_vec();
    let marker = disconnect.clone();
    let stream = unfold((0u8, event, DisconnectGuard(disconnect)), move |state| {
        let marker = marker.clone();
        async move {
            let (phase, event, guard) = state;
            if phase == 0 {
                return Some((
                    Ok::<_, std::io::Error>(Bytes::from(event.clone())),
                    (1, event, guard),
                ));
            }
            #[cfg_attr(
                dylint_lib = "rr_lints",
                expect(
                    sleep,
                    reason = "the fixture holds the stream open past the client's idle timeout; the wait is the fixture"
                )
            )]
            tokio::time::sleep(Duration::from_secs(30)).await;
            if phase == 1 {
                *marker.done.lock() = true;
                return Some((
                    Ok(Bytes::from_static(b"data: [DONE]\n\n")),
                    (2, event, guard),
                ));
            }
            Some((Ok(Bytes::new()), (2, event, guard)))
        }
    });
    streaming_response(stream)
}

async fn streaming_handler(
    axum::extract::State((recorder, body)): axum::extract::State<(Recorder, Arc<String>)>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let call = capture(request).await;
    recorder.lock().push(call);
    let text = body.as_bytes().to_vec();
    let stream = unfold((0usize, text), |state| async move {
        let (index, text) = state;
        if index > 0 {
            #[cfg_attr(
                dylint_lib = "rr_lints",
                expect(
                    sleep,
                    reason = "chunks arrive one at a time; the 20 ms pacing makes the client read them separately"
                )
            )]
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Some((
            Ok::<_, std::io::Error>(Bytes::from(text.clone())),
            (index + 1, text),
        ))
    });
    streaming_response(stream)
}

fn streaming_response(
    stream: impl futures::Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
) -> axum::response::Response {
    demand(
        axum::response::Response::builder()
            .status(axum::http::StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
            .body(Body::from_stream(stream)),
        "streaming response builds",
    )
}

async fn fixed_handler(
    axum::extract::State((recorder, (status, body))): axum::extract::State<(
        Recorder,
        (u16, String),
    )>,
    request: axum::extract::Request,
) -> axum::response::Response {
    let call = capture(request).await;
    recorder.lock().push(call);
    let status = demand(axum::http::StatusCode::from_u16(status), "test status");
    (status, body).into_response()
}

async fn capture(request: axum::extract::Request) -> CapturedCall {
    let (parts, body) = request.into_parts();
    let authorization = parts
        .headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = demand(
        axum::body::to_bytes(body, 8 * 1024 * 1024).await,
        "request body reads",
    );
    CapturedCall {
        method: parts.method.to_string(),
        target: parts.uri.to_string(),
        authorization,
        body: String::from_utf8_lossy(&bytes).to_string(),
    }
}

pub(crate) async fn wait_for(condition: impl Fn() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return condition();
        }
        #[cfg_attr(
            dylint_lib = "rr_lints",
            expect(
                sleep,
                reason = "the stub's state change has no awaitable event; the 5 ms tick is the poll interval bounded by the timeout"
            )
        )]
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

pub(crate) fn canonicalize_arguments(value: &mut Json) {
    match value {
        Json::Object(map) => {
            for (key, member) in map.iter_mut() {
                if key == "arguments"
                    && let Some(text) = member.as_str()
                    && let Ok(parsed) = serde_json::from_str::<Json>(text)
                {
                    *member = Json::String(parsed.to_string());
                }
                canonicalize_arguments(member);
            }
        }
        Json::Array(items) => {
            for item in items {
                canonicalize_arguments(item);
            }
        }
        Json::Null | Json::Bool(_) | Json::Number(_) | Json::String(_) => {}
    }
}

pub(crate) fn canonical_body(body: &str) -> String {
    let mut parsed: Json = demand(serde_json::from_str(body), "recorded body parses");
    canonicalize_arguments(&mut parsed);
    parsed.to_string()
}

pub(crate) fn chat_provider(addr: SocketAddr, max_tokens: Option<u32>) -> OpenAiCompatible {
    let provider = config::provider::OpenaiProvider {
        model: Some("test-model".to_owned()),
        base_url: Some(format!("http://{addr}")),
        api_key: Some("<redacted>".to_owned()),
        max_tokens,
        ..config::provider::OpenaiProvider::default()
    };
    demand(
        OpenAiCompatible::build(config::provider::ProviderType::Vllm, &provider),
        "the vllm provider builds",
    )
}

pub(crate) fn generic_provider(addr: SocketAddr) -> OpenAiCompatible {
    let provider = config::provider::OpenaiProvider {
        model: Some("test-model".to_owned()),
        base_url: Some(format!("http://{addr}")),
        ..config::provider::OpenaiProvider::default()
    };
    demand(
        OpenAiCompatible::build(config::provider::ProviderType::OpenaiCompatible, &provider),
        "the generic provider builds",
    )
}

pub(crate) fn scenario_record(scenario: &str) -> ProviderScenario {
    let dir = testkit::recording(&format!("providers/vllm/{scenario}"));
    let mut recorded = demand(
        testkit::provider::load_scenario(&dir),
        "the recorded scenario loads",
    );
    for request in &mut recorded.requests {
        if request.body.is_empty() {
            continue;
        }
        let stripped = demand(
            testkit::strip_b8_deltas(&request.body),
            "recorded body strips",
        );
        request.body = canonical_body(&stripped);
    }
    if let Some(chunks) = recorded.chunks.as_mut() {
        chunks.retain(|chunk| chunk.get("type").and_then(Json::as_str) != Some("tool_call_done"));
    }
    recorded
}

pub(crate) fn captured_requests(recorder: &Recorder) -> Vec<CapturedRequest> {
    recorder
        .lock()
        .iter()
        .map(|call| CapturedRequest {
            method: call.method.clone(),
            target: call.target.clone(),
            headers: match &call.authorization {
                Some(value) => vec![("authorization".to_owned(), value.clone())],
                None => Vec::new(),
            },
            body: if call.body.is_empty() {
                String::new()
            } else {
                canonical_body(&call.body)
            },
        })
        .collect()
}

include!("scenario.rs");
