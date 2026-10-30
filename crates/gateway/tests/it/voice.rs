use crate::common;

#[path = "voice/proxy.rs"]
mod proxy;
#[path = "voice/upstream.rs"]
mod upstream;

use upstream::Huge;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use kernel::memory::MemoryStore;
use serde::Deserialize;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use wire::gateway::GatewayError;

#[derive(Clone)]
struct Seen {
    hits: Arc<tokio::sync::Mutex<Vec<Hit>>>,
    stop: CancellationToken,
}

struct Hit {
    path: String,
    body: Vec<u8>,
}

#[derive(Serialize)]
struct Heard {
    text: String,
    ignored: bool,
}

#[derive(Deserialize)]
struct Spoken {
    input: String,
    response_format: String,
    #[serde(default)]
    instructions: Option<String>,
    #[serde(default)]
    voice: Option<String>,
}

struct Upstream {
    addr: SocketAddr,
    seen: Seen,
    token: CancellationToken,
    server: tokio::task::JoinHandle<Result<(), std::io::Error>>,
}

fn assert_error(response: &testkit::Response, status: u16, message: &str) -> Result<(), String> {
    assert_eq!(response.status, status, "{}", body_text(&response.body));
    assert_eq!(gateway_error(response)?.error, message);
    Ok(())
}

fn base() -> gateway::Gateway {
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    gateway::Gateway::new(store, common::settings())
}

async fn close(gateway: Arc<gateway::Gateway>) -> Result<(), String> {
    tokio::time::timeout(Duration::from_secs(5), gateway.close())
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn gateway_error(response: &testkit::Response) -> Result<GatewayError, String> {
    serde_json::from_slice(&response.body).map_err(|error| error.to_string())
}

fn body_text(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        text.to_owned()
    } else {
        "body is not utf-8".to_owned()
    }
}

async fn call(
    app: &axum::Router,
    method: &str,
    target: &str,
    body: Vec<u8>,
    extra: &[(&str, &str)],
) -> Result<testkit::Response, String> {
    match tokio::time::timeout(
        Duration::from_secs(5),
        common::answer(app.clone(), recorded(method, target, body, extra)),
    )
    .await
    {
        Ok(response) => response,
        Err(error) => Err(error.to_string()),
    }
}

fn recorded(method: &str, target: &str, body: Vec<u8>, extra: &[(&str, &str)]) -> testkit::Request {
    let mut headers = vec![("host".to_owned(), "127.0.0.1:5174".to_owned())];
    for (name, value) in extra {
        headers.push(((*name).to_owned(), (*value).to_owned()));
    }
    testkit::Request {
        method: method.to_owned(),
        target: target.to_owned(),
        headers,
        body,
        recipe: None,
        via: Some("loopback".to_owned()),
    }
}

async fn post_json(
    app: &axum::Router,
    target: &str,
    body: &impl Serialize,
) -> Result<testkit::Response, String> {
    call(
        app,
        "POST",
        target,
        serde_json::to_vec(body).map_err(|error| error.to_string())?,
        &[("content-type", "application/json")],
    )
    .await
}
