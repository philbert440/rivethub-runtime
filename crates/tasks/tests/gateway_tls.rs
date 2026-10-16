#[expect(
    dead_code,
    reason = "each test binary includes the shared harness and uses a different subset"
)]
#[expect(
    clippy::expect_used,
    reason = "a shared harness reports setup failure by panicking; the message names the step"
)]
mod common;

#[expect(
    dead_code,
    reason = "each test binary includes the certificate mint and uses a different subset"
)]
#[expect(
    clippy::expect_used,
    reason = "certificate minting failure is a broken fixture"
)]
mod mesh_support;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::Path;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::routing::get;
use axum::routing::post;
use chrono::DateTime;
use chrono::Utc;
use serde::Serialize;
use tasks::Error;
use tasks::MeshIdentity;
use tasks::RunOutcome;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use wire::gateway::HealthzResponse;
use wire::gateway::sessions::SessionMessage;
use wire::gateway::sessions::SessionMessageRole;
use wire::gateway::sessions::SessionPostReply;
use wire::task::TaskStatus;

#[derive(Clone)]
struct Observed {
    hits: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct Gate {
    hits: Arc<AtomicUsize>,
    presented: bool,
}

struct Peer {
    port: u16,
    observed: Observed,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Peer {
    async fn shutdown(self) {
        self.cancel.cancel();
        let joined = tokio::time::timeout(Duration::from_secs(2), self.task).await;
        assert!(joined.is_ok());
    }
}

#[expect(
    clippy::expect_used,
    reason = "binding the gateway is fixture setup; a failure is a broken test"
)]
async fn start(material: &mesh_support::Material, require_client: bool) -> Peer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("port").port();
    let observed = Observed {
        hits: Arc::new(AtomicUsize::new(0)),
    };
    let acceptor = acceptor(material, require_client);
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let counts = observed.clone();
    let task = tokio::spawn(async move {
        listen(listener, acceptor, counts, stop).await;
    });
    Peer {
        port,
        observed,
        cancel,
        task,
    }
}

#[expect(
    clippy::expect_used,
    reason = "the gateway certificate is a fixture; a failure is a broken test"
)]
fn acceptor(material: &mesh_support::Material, require_client: bool) -> TlsAcceptor {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(material.ca.certificate.der().clone())
        .expect("gateway root");
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("tls versions");
    let with_client = if require_client {
        let verifier =
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .expect("client verifier");
        builder.with_client_cert_verifier(verifier)
    } else {
        let verifier =
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .allow_unauthenticated()
                .build()
                .expect("optional client verifier");
        builder.with_client_cert_verifier(verifier)
    };
    let mut config = with_client
        .with_single_cert(
            vec![material.server.certificate.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(material.server.key.serialize_der()).into(),
        )
        .expect("gateway server certificate");
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    TlsAcceptor::from(Arc::new(config))
}

fn presented(tls: &tokio_rustls::server::TlsStream<tokio::net::TcpStream>) -> bool {
    tls.get_ref()
        .1
        .peer_certificates()
        .is_some_and(|certs| !certs.is_empty())
}

async fn listen(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    observed: Observed,
    cancel: CancellationToken,
) {
    let mut connections = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { break };
                let acceptor = acceptor.clone();
                let observed = observed.clone();
                connections.spawn(async move {
                    if let Ok(tls) = acceptor.accept(stream).await {
                        let router = gateway(observed, presented(&tls));
                        let served = hyper::server::conn::http1::Builder::new()
                            .serve_connection(
                                hyper_util::rt::TokioIo::new(tls),
                                hyper_util::service::TowerToHyperService::new(router),
                            )
                            .await;
                        drop(served);
                    }
                });
            }
            Some(joined) = connections.join_next(), if !connections.is_empty() => {
                assert!(joined.is_ok());
            }
        }
    }
    connections.abort_all();
}

fn gateway(observed: Observed, presented: bool) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/api/sessions/{session}/messages", post(message))
        .with_state(Gate {
            hits: observed.hits,
            presented,
        })
}

async fn health(State(gate): State<Gate>) -> axum::response::Response {
    gate.hits.fetch_add(1, Ordering::SeqCst);
    json_response(&HealthzResponse {
        ok: true,
        sessions: 0,
        name: "gateway".to_owned(),
        node: "local".to_owned(),
        degraded: None,
        degraded_providers: Vec::new(),
    })
}

async fn message(
    State(gate): State<Gate>,
    Path(_session): Path<String>,
    _body: Bytes,
) -> axum::response::Response {
    gate.hits.fetch_add(1, Ordering::SeqCst);
    if gate.presented {
        return unauthorized();
    }
    json_response(&reply())
}

#[expect(
    clippy::expect_used,
    reason = "the unauthorized document is a fixed fixture"
)]
fn unauthorized() -> axum::response::Response {
    let bytes = serde_json::to_vec(&wire::gateway::GatewayError::new("unauthorized"))
        .expect("unauthorized json");
    axum::response::Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(bytes))
        .expect("unauthorized response")
}

#[expect(
    clippy::expect_used,
    reason = "the gateway reply stamp is a fixed instant; a parse failure is a broken fixture"
)]
fn reply() -> SessionPostReply {
    SessionPostReply {
        message: SessionMessage {
            id: "m1".to_owned(),
            session_id: "s1".to_owned(),
            role: SessionMessageRole::Assistant,
            text: "done".to_owned(),
            ts: DateTime::<Utc>::from_timestamp_millis(1_760_000_001_000).expect("stamp"),
            usage: None,
            model: None,
            duration_ms: None,
            thinking: None,
            tools: None,
        },
    }
}

#[expect(
    clippy::expect_used,
    reason = "the gateway health document is a fixed fixture"
)]
fn json_response(body: &impl Serialize) -> axum::response::Response {
    let bytes = serde_json::to_vec(body).expect("gateway json");
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(bytes))
        .expect("gateway response")
}

#[expect(
    clippy::expect_used,
    reason = "the gateway url is a fixed fixture; a parse failure is a broken test"
)]
fn engine(
    port: u16,
    trust: Option<MeshIdentity>,
) -> tasks::Engine<testkit::TaskMemory, testkit::TaskMemory> {
    let base = url::Url::parse(&format!("https://127.0.0.1:{port}/")).expect("gateway url");
    let mut directory = common::directory(base);
    directory.set_gateway_trust(trust);
    let (engine, _) = common::engine_with(
        directory,
        common::settings(None, std::collections::BTreeMap::new()),
    );
    engine
}

fn chain_contains(error: &Error, needle: &str) -> bool {
    let Error::GatewayTls(source) = error else {
        return false;
    };
    let mut current: Option<&dyn std::error::Error> = Some(source.as_ref());
    while let Some(item) = current {
        if item.to_string().contains(needle) {
            return true;
        }
        current = std::error::Error::source(item);
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_chat_loop_completes_as_the_loopback_owner() {
    let material = mesh_support::mint("node.example.test");
    let peer = start(&material, false).await;
    let trust = MeshIdentity::from_ca(&material.ca.pem).expect("ca");
    assert!(!trust.presents_certificate());
    let engine = engine(peer.port, Some(trust));
    let id = engine
        .submit(common::task("ship it"))
        .await
        .expect("submit");
    let outcome = tokio::time::timeout(Duration::from_secs(5), engine.run_one())
        .await
        .expect("deadline")
        .expect("run");
    assert_eq!(outcome, RunOutcome::Completed(id));
    let loaded = engine.load(id).await.expect("load");
    assert_eq!(loaded.status, TaskStatus::Completed);
    assert_eq!(
        loaded.result.as_ref().map(|result| result.summary.as_str()),
        Some("done")
    );
    assert_eq!(peer.observed.hits.load(Ordering::SeqCst), 2);
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn presenting_the_node_certificate_is_unauthorized() {
    let material = mesh_support::mint("node.example.test");
    let peer = start(&material, false).await;
    let trust = material.identity(&material.ca.pem);
    assert!(trust.presents_certificate());
    let engine = engine(peer.port, Some(trust));
    let id = engine
        .submit(common::task("ship it"))
        .await
        .expect("submit");
    let error = tokio::time::timeout(Duration::from_secs(5), engine.run_one())
        .await
        .expect("deadline")
        .expect_err("node certificate");
    assert!(
        matches!(
            &error,
            Error::Rejected {
                status: 401,
                error: Some(body),
            } if body.error == "unauthorized"
        ),
        "{error}"
    );
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Failed
    );
    assert_eq!(peer.observed.hits.load(Ordering::SeqCst), 2);
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ca_only_trust_completes_when_the_gateway_allows_anonymous_clients() {
    let material = mesh_support::mint("node.example.test");
    let peer = start(&material, false).await;
    let trust = MeshIdentity::from_ca(&material.ca.pem).expect("ca");
    assert!(!trust.presents_certificate());
    let engine = engine(peer.port, Some(trust));
    let id = engine
        .submit(common::task("ship it"))
        .await
        .expect("submit");
    let outcome = tokio::time::timeout(Duration::from_secs(5), engine.run_one())
        .await
        .expect("deadline")
        .expect("run");
    assert_eq!(outcome, RunOutcome::Completed(id));
    assert_eq!(peer.observed.hits.load(Ordering::SeqCst), 2);
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_gateway_certificate_from_another_ca_fails_typed() {
    let mesh = mesh_support::mint("node.example.test");
    let foreign = mesh_support::mint("foreign.example.test");
    let peer = start(&foreign, false).await;
    let engine = engine(
        peer.port,
        Some(MeshIdentity::from_ca(&mesh.ca.pem).expect("ca")),
    );
    let id = engine
        .submit(common::task("ship it"))
        .await
        .expect("submit");
    let error = tokio::time::timeout(Duration::from_secs(5), engine.run_one())
        .await
        .expect("deadline")
        .expect_err("issuer");
    assert!(matches!(error, Error::GatewayTls(_)));
    assert_eq!(error.to_string(), "gateway tls verification failed");
    assert!(chain_contains(
        &error,
        "invalid peer certificate: UnknownIssuer"
    ));
    assert_eq!(peer.observed.hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Failed
    );
    peer.shutdown().await;
}

#[tokio::test]
async fn an_https_gateway_without_trust_is_refused_before_dial() {
    let engine = engine(9, None);
    let error = engine
        .submit(common::task("ship it"))
        .await
        .expect_err("trust");
    assert!(matches!(error, Error::GatewayTrustRequired));
}
