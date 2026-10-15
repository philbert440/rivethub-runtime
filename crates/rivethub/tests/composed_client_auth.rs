use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Method;
use axum::http::Request;
use axum::http::StatusCode;
use kernel::memory::MemoryStore;
use rivethub::composition::Composition;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const REMOTE: &str = "192.0.2.7:41000";

fn write_config(state: &Path, work: &Path) -> TestResult {
    let config = format!(
        "runtime:\n  workspace: {work}\n  default_agent: main\nagents:\n  main:\n    provider: p\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:9/v1\n    model: m\n    probe_models: false\nden:\n  host: 127.0.0.1\n  port: 5975\n",
        work = work.display(),
    );
    std::fs::write(state.join("config.yaml"), config)?;
    Ok(())
}

async fn status(
    router: axum::Router,
    method: Method,
    path: &str,
    origin: Option<&str>,
) -> TestResult<StatusCode> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "127.0.0.1:5975");
    if let Some(origin) = origin {
        builder = builder
            .header("origin", origin)
            .header("access-control-request-method", "GET");
    }
    let mut request = builder.body(Body::empty())?;
    request
        .extensions_mut()
        .insert(ConnectInfo(REMOTE.parse::<SocketAddr>()?));
    Ok(router.oneshot(request).await?.status())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_composed_router_keeps_the_open_routes_open_without_a_client_certificate() -> TestResult
{
    let home = tempfile::tempdir()?;
    let work = home.path().join("work");
    let state = home.path().join(".rivethub");
    std::fs::create_dir_all(&work)?;
    std::fs::create_dir_all(&state)?;
    write_config(&state, &work)?;
    let env = config::Environment::from_vars([("HOME", home.path().to_str().ok_or("home")?)]);
    let loaded = config::Config::load(
        &env,
        &config::ConfigLocation::Current(state.join("config.yaml")),
    )?;
    let shutdown = CancellationToken::new();
    let store: Arc<dyn MemoryStore> = Arc::new(testkit::FakeMemoryStore::default());
    let composition = tokio::time::timeout(
        Duration::from_secs(30),
        Composition::build(&loaded.config, &env, store, &shutdown),
    )
    .await??;
    let router = composition.router(true);
    assert_eq!(
        status(router.clone(), Method::GET, "/healthz", None).await?,
        StatusCode::OK
    );
    for path in ["/ca/v1/chain", "/ca/v1/crl", "/api/devices/pair"] {
        let answered = status(router.clone(), Method::GET, path, None).await?;
        assert_ne!(answered, StatusCode::UNAUTHORIZED, "{path}");
    }
    assert_eq!(
        status(
            router.clone(),
            Method::OPTIONS,
            "/sessions",
            Some("app://bundle")
        )
        .await?,
        StatusCode::NO_CONTENT
    );
    for path in ["/sessions", "/api/agents", "/api/memory/search?q=x"] {
        let refused = status(router.clone(), Method::GET, path, None).await?;
        assert_eq!(refused, StatusCode::UNAUTHORIZED, "{path}");
    }
    shutdown.cancel();
    composition.close().await?;
    Ok(())
}
