use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Method;
use axum::http::Request;
use axum::http::StatusCode;
use axum::http::header;
use gateway::tenancy::PeerCertificate;
use kernel::memory::MemoryStore;
use rivethub::composition::Composition;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const REMOTE: &str = "192.0.2.7:41000";
const USERS: &str = r#"{"ownerUserId":"owner","unmappedIsOwner":false,"users":{"owner":{"devices":["known-phone"]}}}"#;

struct Composed {
    router: axum::Router,
    composition: Composition,
    shutdown: CancellationToken,
    _home: tempfile::TempDir,
}

impl Composed {
    async fn build(users: Option<&str>) -> TestResult<Self> {
        let home = tempfile::tempdir()?;
        let work = home.path().join("work");
        let state = home.path().join(".rivethub");
        std::fs::create_dir_all(&work)?;
        std::fs::create_dir_all(&state)?;
        write_config(&state, &work, users)?;
        let env = config::Environment::from_vars([("HOME", home.path().to_str().ok_or("home")?)]);
        let loaded = config::Config::load(
            &env,
            &config::ConfigLocation::Current(state.join("config.yaml")),
            config::Profile::Node,
        )?;
        let shutdown = CancellationToken::new();
        let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
        let composition = tokio::time::timeout(
            Duration::from_secs(30),
            Composition::build(&loaded.config, &env, store, &shutdown),
        )
        .await??;
        Ok(Self {
            router: composition.router(true),
            composition,
            shutdown,
            _home: home,
        })
    }

    async fn close(self) -> TestResult {
        self.shutdown.cancel();
        self.composition.close().await?;
        Ok(())
    }
}

fn write_config(state: &Path, work: &Path, users: Option<&str>) -> TestResult {
    let users_line = match users {
        Some(body) => {
            let path = state.join("users.json");
            std::fs::write(&path, body)?;
            format!("  users_file: {}\n", path.display())
        }
        None => String::new(),
    };
    let config = format!(
        "runtime:\n  workspace: {work}\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:9/v1\n    model: m\n    probe_models: false\nden:\n  host: 127.0.0.1\n  port: 5975\n{users_line}",
        work = work.display(),
    );
    std::fs::write(state.join("config.yaml"), config)?;
    Ok(())
}

struct Call<'a> {
    method: Method,
    path: &'a str,
    peer: Option<PeerCertificate>,
    headers: &'a [(header::HeaderName, &'a str)],
}

impl<'a> Call<'a> {
    fn get(path: &'a str) -> Self {
        Self {
            method: Method::GET,
            path,
            peer: None,
            headers: &[],
        }
    }

    async fn send(self, router: &axum::Router) -> TestResult<axum::response::Response> {
        let mut builder = Request::builder()
            .method(self.method)
            .uri(self.path)
            .header(header::HOST, "127.0.0.1:5975");
        for (name, value) in self.headers {
            builder = builder.header(name, *value);
        }
        let mut request = builder.body(Body::empty())?;
        request
            .extensions_mut()
            .insert(ConnectInfo(REMOTE.parse::<SocketAddr>()?));
        if let Some(peer) = self.peer {
            request.extensions_mut().insert(peer);
        }
        Ok(router.clone().oneshot(request).await?)
    }
}

fn device(name: &str) -> PeerCertificate {
    PeerCertificate::Device {
        user: Some("owner".to_owned()),
        device: name.to_owned(),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_routes_answer_a_caller_without_a_client_certificate() -> TestResult {
    let composed = Composed::build(None).await?;
    let healthz = Call::get("/healthz").send(&composed.router).await?;
    assert_eq!(healthz.status(), StatusCode::OK);
    for path in ["/ca/v1/chain", "/ca/v1/crl", "/api/devices/pair"] {
        let answered = Call::get(path).send(&composed.router).await?;
        assert_ne!(answered.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    let preflight = Call {
        method: Method::OPTIONS,
        path: "/sessions",
        peer: None,
        headers: &[
            (header::ORIGIN, "app://bundle"),
            (header::ACCESS_CONTROL_REQUEST_METHOD, "GET"),
        ],
    }
    .send(&composed.router)
    .await?;
    assert_eq!(preflight.status(), StatusCode::NO_CONTENT);
    composed.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caller_without_a_certificate_gets_the_guards_refusal() -> TestResult {
    let composed = Composed::build(None).await?;
    for path in ["/sessions", "/api/agents", "/api/memory/search?q=x"] {
        let refused = Call {
            method: Method::GET,
            path,
            peer: None,
            headers: &[(header::ORIGIN, "app://bundle")],
        }
        .send(&composed.router)
        .await?;
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED, "{path}");
        assert!(
            refused
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            "{path} keeps CORS"
        );
    }
    let browser = Call {
        method: Method::GET,
        path: "/",
        peer: None,
        headers: &[(header::ACCEPT, "text/html")],
    }
    .send(&composed.router)
    .await?;
    assert_eq!(browser.status(), StatusCode::UNAUTHORIZED);
    let content_type = browser
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or("content type")?;
    assert!(content_type.starts_with("text/html"), "{content_type}");
    composed.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_users_registry_a_verified_device_is_the_owner() -> TestResult {
    let composed = Composed::build(None).await?;
    let mut call = Call::get("/sessions");
    call.peer = Some(device("any-phone"));
    assert_eq!(call.send(&composed.router).await?.status(), StatusCode::OK);
    let mut node = Call::get("/sessions");
    node.peer = Some(PeerCertificate::Node {
        node: "node-a".to_owned(),
    });
    assert_eq!(
        node.send(&composed.router).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    composed.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_a_users_registry_an_unknown_device_is_refused() -> TestResult {
    let composed = Composed::build(Some(USERS)).await?;
    let mut known = Call::get("/sessions");
    known.peer = Some(device("known-phone"));
    assert_eq!(known.send(&composed.router).await?.status(), StatusCode::OK);
    let mut unknown = Call::get("/sessions");
    unknown.peer = Some(device("stranger-phone"));
    assert_eq!(
        unknown.send(&composed.router).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    composed.close().await
}
