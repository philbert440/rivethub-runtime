use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Request;
use axum::response::Response;
use gateway::routes::v1::Service;
use gateway::{Gateway, Settings};
use kernel::model::{ModelChunk, ModelProvider};
use serde::de::DeserializeOwned;
use tempfile::TempDir;
use testkit::{FakeMemoryStore, FakeProvider, Script};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use wire::stream::LlmUsage;

pub(super) struct Harness {
    pub(super) service: Arc<Service>,
    pub(super) gateway: Arc<Gateway>,
    pub(super) dir: TempDir,
    pub(super) runner: Arc<super::runner::FakeRunner>,
    pub(super) provider: Arc<dyn ModelProvider>,
}

impl Harness {
    pub(super) async fn new(chunks: Vec<ModelChunk>) -> Self {
        Self::provider(Arc::new(FakeProvider::new(vec![
            Script::Stream(chunks),
            Script::Models(vec!["test-model".to_owned()]),
        ])))
        .await
    }

    pub(super) async fn provider(provider: Arc<dyn ModelProvider>) -> Self {
        let dir = tempfile::tempdir().expect("scratch directory");
        let runner = Arc::new(super::runner::FakeRunner::new(Arc::clone(&provider)));
        let service = Service::open(
            "test-provider".to_owned(),
            Arc::clone(&provider),
            &dir.path().join("conversations"),
            CancellationToken::new(),
        )
        .await
        .expect("service opens");
        let gateway = Arc::new(
            Gateway::new(
                Arc::new(FakeMemoryStore::default()),
                Settings {
                    hostname: "example.test".to_owned(),
                    node: "test-node".to_owned(),
                    allowed_origins: Vec::new(),
                    allowed_hosts: Vec::new(),
                    files_root: None,
                    static_dir: None,
                    root_redirect: None,
                    state_dir: dir.path().join("state"),
                    directory_root: dir.path().join("agents"),
                    mesh_file: Some(dir.path().join("mesh.json")),
                    shared_dir: dir.path().join("shared"),
                    legacy_mesh_dir: dir.path().join("legacy"),
                    allowed_harnesses: None,
                    devices_enabled: false,
                    probe_timeout: Duration::from_millis(200),
                    mesh_cache_ttl: Duration::ZERO,
                    mesh_ca: None,
                },
            )
            .with_turn_runner(runner.clone()),
        );
        Self {
            service,
            gateway,
            dir,
            runner,
            provider,
        }
    }

    pub(super) fn app(&self) -> Router {
        self.service.router(&self.gateway, false)
    }

    pub(super) async fn restart(&mut self) {
        self.service.shutdown().await;
        self.service = Service::open(
            "test-provider".to_owned(),
            Arc::clone(&self.provider),
            &self.dir.path().join("conversations"),
            CancellationToken::new(),
        )
        .await
        .expect("reopen");
    }

    pub(super) async fn close(self) {
        tokio::time::timeout(Duration::from_secs(5), self.service.shutdown())
            .await
            .expect("service joins");
    }
}

pub(super) fn chunks() -> Vec<ModelChunk> {
    vec![
        ModelChunk::Reasoning {
            delta: "consider".to_owned(),
        },
        ModelChunk::Text {
            delta: "Hello 🌍".to_owned(),
        },
        ModelChunk::Usage {
            usage: LlmUsage {
                prompt_tokens: 3,
                completion_tokens: 2,
                reasoning_tokens: None,
                cached_tokens: None,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        },
        ModelChunk::Done,
    ]
}

pub(super) async fn request(
    app: Router,
    method: &str,
    path: &str,
    body: &str,
    headers: &[(&str, &str)],
) -> Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("content-type", "application/json")
        .extension(ConnectInfo(
            "127.0.0.1:41000".parse::<SocketAddr>().expect("address"),
        ));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    tokio::time::timeout(
        Duration::from_secs(5),
        app.oneshot(request.body(Body::from(body.to_owned())).expect("request")),
    )
    .await
    .expect("response bounded")
    .expect("response")
}

pub(super) async fn bytes(response: Response) -> axum::body::Bytes {
    tokio::time::timeout(
        Duration::from_secs(5),
        axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024),
    )
    .await
    .expect("body bounded")
    .expect("body")
}

pub(super) async fn json<T: DeserializeOwned>(response: Response) -> T {
    serde_json::from_slice(&bytes(response).await).expect("typed response")
}

pub(super) const REQUEST: &str =
    r#"{"model":"test-model","messages":[{"role":"user","content":"Hi"}]}"#;
