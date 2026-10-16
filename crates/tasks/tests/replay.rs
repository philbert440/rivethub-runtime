#[expect(
    dead_code,
    reason = "each test binary includes the shared harness and uses a different subset"
)]
#[expect(
    clippy::expect_used,
    reason = "a shared harness reports setup failure by panicking; the message names the step"
)]
mod common;

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

#[derive(Deserialize)]
struct PgFile {
    scenarios: Vec<PgScenario>,
}

#[derive(Deserialize)]
struct PgScenario {
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
    method: String,
    target: String,
    status: u16,
    content_type: String,
    body: String,
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

fn content_type(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default()
}

#[expect(
    clippy::expect_used,
    reason = "a request head that does not parse is a broken fixture"
)]
fn head(case: &testkit::http::HttpCase) -> RequestHead {
    testkit::http::parse_head(&case.request.head).expect("request head")
}

#[expect(
    clippy::expect_used,
    reason = "the vendored gateway recording is part of the crate; a missing file is a broken fixture"
)]
fn exchanges() -> Vec<Exchange> {
    let cases = testkit::http::cases(&golden().join("http-tls.json")).expect("http golden");
    cases
        .into_iter()
        .map(|case| {
            let parsed = head(&case);
            Exchange {
                method: parsed.method,
                target: parsed.target,
                status: testkit::http::status_of(&case.response.status_line)
                    .expect("recorded HTTP status"),
                content_type: content_type(&case.response.headers),
                body: case.response.body.unwrap_or_default(),
            }
        })
        .collect()
}

#[expect(
    clippy::expect_used,
    reason = "the vendored graphile recording is part of the crate; a missing file is a broken fixture"
)]
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

#[expect(
    clippy::expect_used,
    reason = "a replay compares the typed document; a decode or client failure fails the case"
)]
#[expect(
    clippy::panic,
    reason = "an in-slice exchange that is not health or sessions, or an unexpected error variant, fails the case"
)]
async fn replay_exchange(exchange: &Exchange) {
    let stub = recorded_stub(exchange).await;
    let client = GatewayClient::new(common::base_url(stub.addr)).expect("client");
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
    stub.close().await.expect("stub shuts down");
}

#[expect(
    clippy::expect_used,
    reason = "the local stub must bind before a replay can compare the document"
)]
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

#[expect(
    clippy::expect_used,
    reason = "a status outside the http range is a broken fixture"
)]
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
