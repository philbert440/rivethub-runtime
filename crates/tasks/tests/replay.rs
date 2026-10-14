#![expect(
    clippy::expect_used,
    reason = "allow-expect-in-tests covers only #[test] functions; helpers panic when a fixture or a local stub fails to set up"
)]
#![expect(
    clippy::panic,
    reason = "a replay helper panics when a recorded document is outside the cases the client can compare"
)]

mod common;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use kernel::queue::FixedJob;
use kernel::queue::JobName;
use serde::Deserialize;
use tasks::Error;
use tasks::GatewayClient;
use testkit::http::RequestHead;
use wire::events::SessionList;
use wire::gateway::GatewayError;
use wire::gateway::HealthzResponse;

const CORS: &str = "CORS preflight is the gateway, not a task session call";
const HTML: &str = "the task client reads JSON wire documents, not HTML pages";
const DEVICE: &str = "device enrollment and phone pairing belong to the gateway";
const EVENT: &str = "capture event ingest is not a task session call";
const STATE: &str = "registry state and deletion stay on the gateway";
const OTHER: &str = "outside the task client's gateway surface of GET /healthz and GET /sessions";
const SOCKET: &str =
    "the task client speaks the gateway HTTP API; this step is a websocket session stream";
const TENANCY: &str = "tenancy user directory is gateway boot input, not a task HTTP exchange";
const INVALID: &str = "truncated users document is gateway boot input, not a task HTTP exchange";

#[derive(Deserialize)]
struct Skip {
    name: String,
    reason: String,
}

#[derive(Deserialize)]
struct PgFile {
    scenarios: Vec<PgScenario>,
}

#[derive(Deserialize)]
struct PgScenario {
    name: String,
    schema: String,
    steps: Vec<PgStep>,
}

#[derive(Deserialize)]
struct PgStep {
    rows: Option<PgRows>,
}

#[derive(Deserialize)]
struct PgRows {
    #[serde(default)]
    graphile_jobs: Vec<GraphileJob>,
}

#[derive(Deserialize)]
struct GraphileJob {
    task: String,
    queue: Option<String>,
    payload: String,
    key: Option<String>,
    priority: i64,
    max_attempts: i64,
    attempts: i64,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct EmbedTarget {
    target_table: String,
    target_id: String,
}

struct Exchange {
    name: String,
    method: String,
    target: String,
    status: u16,
    content_type: String,
    body: String,
}

fn skips() -> Vec<Skip> {
    serde_json::from_str(include_str!("fixtures/skips.json")).expect("skip list")
}

fn golden() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../wire/tests/golden/gateway")
}

fn capture() -> PathBuf {
    testkit::recording("capture/pg")
}

fn in_slice(method: &str, target: &str, content_type: &str, body: &str) -> bool {
    method == "GET"
        && (target == "/healthz" || target == "/sessions")
        && content_type
            .to_ascii_lowercase()
            .starts_with("application/json")
        && !body.is_empty()
}

fn http_reason(method: &str, target: &str, content_type: &str) -> &'static str {
    if method == "OPTIONS" {
        return CORS;
    }
    if content_type.to_ascii_lowercase().starts_with("text/html")
        || target == "/"
        || target == "/index.html"
    {
        return HTML;
    }
    if target.starts_with("/api/devices") || target.starts_with("/api/phone") {
        return DEVICE;
    }
    if method == "POST" && (target == "/event" || target.starts_with("/event?")) {
        return EVENT;
    }
    if target.starts_with("/state") || method == "DELETE" {
        return STATE;
    }
    OTHER
}

fn file_reason(name: &str) -> &'static str {
    match name {
        "users-tenancy.json" => TENANCY,
        "users-invalid.json" => INVALID,
        _ => OTHER,
    }
}

fn scratch_reason(schema: &str) -> String {
    format!(
        "scratch schema {schema} has no graphile job rows; the task queue contract is the public-schema dump"
    )
}

fn content_type(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

fn head(case: &testkit::http::HttpCase) -> RequestHead {
    testkit::http::parse_head(&case.request.head).expect("request head")
}

fn exchanges() -> Vec<Exchange> {
    let cases = testkit::http::cases(&golden().join("http-tls.json")).expect("http golden");
    cases
        .into_iter()
        .map(|case| {
            let parsed = head(&case);
            let id = case
                .id
                .split_once('/')
                .map_or_else(|| case.id.clone(), |(_, id)| id.to_string());
            Exchange {
                name: format!("http/{id}"),
                method: parsed.method,
                target: parsed.target,
                status: testkit::http::status_of(&case.response.status_line),
                content_type: content_type(&case.response.headers),
                body: case.response.body.unwrap_or_default(),
            }
        })
        .collect()
}

fn ws_names() -> Vec<String> {
    testkit::ws::steps(&golden().join("ws.json"))
        .expect("ws golden")
        .into_iter()
        .map(|step| {
            let id = match step.id.split_once('/') {
                Some((_, id)) => id.to_string(),
                None => step.id,
            };
            format!("ws/{id}")
        })
        .collect()
}

fn pg_files() -> Vec<PgFile> {
    let mut paths = std::fs::read_dir(capture())
        .expect("pg dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let text = std::fs::read_to_string(&path).expect("pg file");
            serde_json::from_str(&text).expect("pg scenarios")
        })
        .collect()
}

fn user_files() -> Vec<String> {
    let mut names = std::fs::read_dir(golden())
        .expect("golden dir")
        .map(|entry| entry.expect("entry").file_name())
        .filter_map(|name| name.to_str().map(ToString::to_string))
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                && name != "http-tls.json"
                && name != "ws.json"
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn expected_skips() -> BTreeMap<String, String> {
    let mut skips = BTreeMap::new();
    for exchange in exchanges() {
        if in_slice(
            &exchange.method,
            &exchange.target,
            &exchange.content_type,
            &exchange.body,
        ) {
            continue;
        }
        skips.insert(
            exchange.name,
            http_reason(&exchange.method, &exchange.target, &exchange.content_type).to_string(),
        );
    }
    for name in ws_names() {
        skips.insert(name, SOCKET.to_string());
    }
    for file in pg_files() {
        for scenario in file.scenarios {
            if scenario.schema == "public" {
                continue;
            }
            skips.insert(
                format!("pg/{}", scenario.name),
                scratch_reason(&scenario.schema),
            );
        }
    }
    for name in user_files() {
        skips.insert(format!("file/{name}"), file_reason(&name).to_string());
    }
    skips
}

#[test]
fn skip_list_names_every_out_of_slice_recording_once() {
    let expected = expected_skips();
    let listed = skips();
    let mut seen = BTreeSet::new();
    for entry in &listed {
        assert!(seen.insert(entry.name.clone()), "duplicate {}", entry.name);
        assert!(!entry.reason.trim().is_empty(), "empty reason");
        assert_eq!(
            expected.get(&entry.name).map(String::as_str),
            Some(entry.reason.as_str()),
            "{}",
            entry.name
        );
    }
    assert_eq!(seen.len(), expected.len());
    let mut replayed = 0;
    for exchange in exchanges() {
        if in_slice(
            &exchange.method,
            &exchange.target,
            &exchange.content_type,
            &exchange.body,
        ) {
            assert!(!seen.contains(&exchange.name), "in-slice skip");
            replayed += 1;
        }
    }
    assert!(replayed > 0);
    for file in pg_files() {
        for scenario in file.scenarios {
            let name = format!("pg/{}", scenario.name);
            if scenario.schema == "public" {
                assert!(!seen.contains(&name));
            } else {
                assert!(seen.contains(&name));
            }
        }
    }
}

#[tokio::test]
async fn replay_gateway_health_and_session_documents() {
    let mut replayed = 0;
    for exchange in exchanges() {
        if !in_slice(
            &exchange.method,
            &exchange.target,
            &exchange.content_type,
            &exchange.body,
        ) {
            continue;
        }
        replay_exchange(&exchange).await;
        replayed += 1;
    }
    assert!(replayed > 0);
}

async fn replay_exchange(exchange: &Exchange) {
    let stub = recorded_stub(exchange).await;
    let client = GatewayClient::new(common::base_url(stub.addr), None).expect("client");
    let bytes = exchange.body.as_bytes();
    if exchange.status < 400 && exchange.target == "/healthz" {
        let expected: HealthzResponse = serde_json::from_slice(bytes).expect("health document");
        assert_eq!(client.healthz().await.expect("health"), expected);
    } else if exchange.status < 400 && exchange.target == "/sessions" {
        let expected: SessionList = serde_json::from_slice(bytes).expect("session document");
        assert_eq!(client.sessions().await.expect("sessions"), expected);
    } else {
        let expected: GatewayError = serde_json::from_slice(bytes).expect("error document");
        let error = match exchange.target.as_str() {
            "/healthz" => client.healthz().await.expect_err("rejected"),
            "/sessions" => client.sessions().await.expect_err("rejected"),
            _ => panic!("in-slice target"),
        };
        let Error::Rejected {
            status,
            error: Some(document),
        } = error
        else {
            panic!("unexpected error: {error:?}");
        };
        assert_eq!(status, exchange.status);
        assert_eq!(*document, expected);
    }
    stub.close().await;
}

async fn recorded_stub(exchange: &Exchange) -> testkit::stub::Stub {
    let status = exchange.status;
    let content_type = exchange.content_type.clone();
    let body = exchange.body.clone();
    let app = axum::Router::new()
        .route(
            "/healthz",
            axum::routing::get(move || {
                let content_type = content_type.clone();
                let body = body.clone();
                async move { recorded(status, content_type, body) }
            }),
        )
        .route(
            "/sessions",
            axum::routing::get({
                let content_type = exchange.content_type.clone();
                let body = exchange.body.clone();
                move || {
                    let content_type = content_type.clone();
                    let body = body.clone();
                    async move { recorded(status, content_type, body) }
                }
            }),
        );
    testkit::stub::stub_server(app).await.expect("stub")
}

fn recorded(status: u16, content_type: String, body: String) -> axum::response::Response {
    axum::response::Response::builder()
        .status(axum::http::StatusCode::from_u16(status).expect("status"))
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .body(axum::body::Body::from(body))
        .expect("response")
}

#[test]
fn replay_public_graphile_rows() {
    let mut jobs = 0;
    for file in pg_files() {
        for scenario in file.scenarios {
            if scenario.schema != "public" {
                continue;
            }
            for step in scenario.steps {
                let rows = step.rows.expect("rows");
                for job in rows.graphile_jobs {
                    let name = job.task.parse::<JobName>().expect("job name");
                    assert_eq!(name, JobName::Fixed(FixedJob::EmbedTarget));
                    assert!(job.queue.is_none());
                    assert_eq!(job.priority, 0);
                    assert_eq!(job.max_attempts, 5);
                    assert_eq!(job.attempts, 0);
                    let payload: EmbedTarget = serde_json::from_str(&job.payload).expect("payload");
                    assert_eq!(
                        job.key.as_deref(),
                        Some(
                            format!("embed-{}-{}", payload.target_table, payload.target_id)
                                .as_str()
                        )
                    );
                    jobs += 1;
                }
            }
        }
    }
    assert!(jobs > 0);
}
