use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use kernel::model::{Embedding, ModelProvider, ModelProviderError, ModelRequest, ModelStreamItem};
use parking_lot::Mutex;
use testkit::{FakeProvider, Script};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt as _;
use wire::gateway::v1::{Conversation, Conversations};
use wire::stream::LlmResponse;

use super::support::{Harness, REQUEST, chunks, json, request};

#[tokio::test]
async fn conversation_crud_survives_restart_and_uses_char_limits() {
    let mut harness = Harness::new(chunks()).await;
    let title = wire::gateway::v1::ConversationTitle {
        title: "🌍".repeat(200),
    };
    let body = serde_json::to_string(&title).expect("title");
    let record: Conversation =
        json(request(harness.app(), "POST", "/api/conversations", &body, &[]).await).await;
    let path = format!("/api/conversations/{}", record.id);
    assert_eq!(record.title, title.title);
    harness.restart().await;
    let loaded: Conversation = json(request(harness.app(), "GET", &path, "", &[]).await).await;
    assert_eq!(loaded, record);
    let renamed: Conversation = json(
        request(
            harness.app(),
            "PATCH",
            &path,
            r#"{"title":"new title"}"#,
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(renamed.title, "new title");
    assert_eq!(renamed.created_at, record.created_at);
    let completion = request(
        harness.app(),
        "POST",
        "/v1/chat/completions",
        REQUEST,
        &[("x-rivet-conversation", &record.id)],
    )
    .await;
    assert_eq!(completion.status(), StatusCode::OK);
    harness.restart().await;
    let loaded: Conversation = json(request(harness.app(), "GET", &path, "", &[]).await).await;
    assert_eq!(loaded.messages.len(), 2);
    assert_eq!(loaded.title, "new title");
    assert_eq!(
        request(harness.app(), "DELETE", &path, "", &[])
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        request(harness.app(), "GET", &path, "", &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    harness.restart().await;
    let listed: Conversations =
        json(request(harness.app(), "GET", "/api/conversations", "", &[]).await).await;
    assert_eq!(listed.conversations, Vec::new());
    harness.close().await;
}

#[tokio::test]
async fn existing_auth_guards_every_new_route_and_service_instances_are_isolated() {
    let first = Harness::new(chunks()).await;
    let second = Harness::new(chunks()).await;
    for path in [
        "/v1/models",
        "/v1/chat/completions",
        "/api/conversations",
        "/api/conversations/00000000-0000-4000-8000-000000000001",
    ] {
        let foreign = request(
            first.app(),
            "GET",
            path,
            "",
            &[("origin", "https://evil.example")],
        )
        .await;
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        let remote = Request::builder()
            .uri(path)
            .header("host", "localhost")
            .extension(ConnectInfo(
                "127.0.0.2:40000".parse::<SocketAddr>().expect("remote"),
            ))
            .body(Body::empty())
            .expect("remote request");
        assert_eq!(
            first
                .app()
                .oneshot(remote)
                .await
                .expect("response")
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let valid = request(
        first.app(),
        "GET",
        "/v1/models",
        "",
        &[("origin", "http://localhost")],
    )
    .await;
    assert_eq!(valid.status(), StatusCode::OK);
    assert_eq!(
        valid
            .headers()
            .get("access-control-allow-origin")
            .expect("CORS"),
        "http://localhost"
    );
    let record: Conversation = json(
        request(
            first.app(),
            "POST",
            "/api/conversations",
            r#"{"title":"private"}"#,
            &[],
        )
        .await,
    )
    .await;
    let response = request(
        second.app(),
        "GET",
        &format!("/api/conversations/{}", record.id),
        "",
        &[("x-rivetos-user", "owner")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        request(first.gateway.router(false), "GET", "/v1/models", "", &[])
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn traversal_invalid_ids_and_symlink_records_are_refused() {
    let harness = Harness::new(chunks()).await;
    for id in [
        "..",
        "../sibling",
        "/etc/passwd",
        "00000000-0000-4000-8000-000000000001/sibling",
    ] {
        let response = request(
            harness.app(),
            "POST",
            "/v1/chat/completions",
            REQUEST,
            &[("x-rivet-conversation", id)],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let directory = harness.dir.path().join("conversations");
    let outside = harness.dir.path().join("conversations-sibling");
    tokio::fs::create_dir(&outside).await.expect("sibling");
    let id = "00000000-0000-4000-8000-000000000001";
    let target = outside.join(format!("{id}.json"));
    tokio::fs::write(&target, "private")
        .await
        .expect("sibling file");
    tokio::fs::symlink(&target, directory.join(format!("{id}.json")))
        .await
        .expect("symlink");
    let open = gateway::routes::v1::Service::open(
        "test-model".to_owned(),
        &directory,
        CancellationToken::new(),
        std::collections::BTreeMap::new(),
    )
    .await;
    assert!(matches!(open, Err(gateway::routes::v1::Error::Invalid(_))));
    assert_eq!(
        tokio::fs::read_to_string(&target)
            .await
            .expect("sibling untouched"),
        "private"
    );
    let traversal = directory.join("../conversations-sibling");
    let open = gateway::routes::v1::Service::open(
        "test-model".to_owned(),
        &traversal,
        CancellationToken::new(),
        std::collections::BTreeMap::new(),
    )
    .await;
    assert!(matches!(open, Err(gateway::routes::v1::Error::Invalid(_))));
    harness.close().await;
}

struct Controlled {
    inner: FakeProvider,
    stream: Mutex<Option<mpsc::Receiver<ModelStreamItem>>>,
    started: mpsc::Sender<ModelRequest>,
}

#[async_trait::async_trait]
impl ModelProvider for Controlled {
    async fn chat(&self, request: ModelRequest) -> Result<LlmResponse, ModelProviderError> {
        self.inner.chat(request).await
    }

    async fn stream(
        &self,
        request: ModelRequest,
    ) -> Result<mpsc::Receiver<ModelStreamItem>, ModelProviderError> {
        let receiver = self.stream.lock().take();
        if let Some(receiver) = receiver {
            self.started
                .send(request)
                .await
                .expect("test observes start");
            Ok(receiver)
        } else {
            self.inner.stream(request).await
        }
    }

    async fn embed(&self, texts: Vec<String>) -> Result<Vec<Embedding>, ModelProviderError> {
        self.inner.embed(texts).await
    }

    async fn list_models(&self) -> Result<Vec<String>, ModelProviderError> {
        self.inner.list_models().await
    }
}

#[tokio::test]
async fn disconnect_cancels_provider_and_unrelated_turns_continue() {
    let (sender, receiver) = mpsc::channel(1);
    let (started, mut requests) = mpsc::channel(1);
    let provider = Arc::new(Controlled {
        inner: FakeProvider::new(vec![Script::Stream(chunks())]),
        stream: Mutex::new(Some(receiver)),
        started,
    });
    let harness = Harness::provider(provider).await;
    let response = request(harness.app(), "POST", "/v1/chat/completions", r#"{"model":"test-model","messages":[{"role":"user","content":"wait"}],"stream":true,"max_tokens":7}"#, &[]).await;
    let first = tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .expect("turn starts")
        .expect("request");
    assert_eq!(first.model, "test-model");
    assert_eq!(first.max_tokens, Some(7));
    assert_eq!(first.tools, Vec::new());
    let id = response
        .headers()
        .get("x-rivet-conversation")
        .expect("id")
        .to_str()
        .expect("id text");
    let conflict = request(
        harness.app(),
        "POST",
        "/v1/chat/completions",
        REQUEST,
        &[("x-rivet-conversation", id)],
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let other = request(harness.app(), "POST", "/v1/chat/completions", REQUEST, &[]).await;
    assert_eq!(other.status(), StatusCode::OK);
    drop(response);
    tokio::time::timeout(Duration::from_secs(5), sender.closed())
        .await
        .expect("provider receiver dropped");
    harness.close().await;
}

#[tokio::test]
async fn shutdown_cancels_a_stalled_turn_and_joins_the_conversation_workers() {
    let (sender, receiver) = mpsc::channel(1);
    let (started, mut requests) = mpsc::channel(1);
    let provider = Arc::new(Controlled {
        inner: FakeProvider::default(),
        stream: Mutex::new(Some(receiver)),
        started,
    });
    let harness = Harness::provider(provider).await;
    let response = request(
        harness.app(),
        "POST",
        "/v1/chat/completions",
        r#"{"model":"test-model","messages":[{"role":"user","content":"wait"}],"stream":true}"#,
        &[],
    )
    .await;
    tokio::time::timeout(Duration::from_secs(5), requests.recv())
        .await
        .expect("turn starts")
        .expect("request");
    tokio::time::timeout(Duration::from_secs(5), harness.service.shutdown())
        .await
        .expect("shutdown finishes");
    assert!(sender.is_closed());
    assert_eq!(
        request(harness.app(), "GET", "/v1/models", "", &[])
            .await
            .status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    drop(response);
    harness.close().await;
}
