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
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::Path;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header;
use axum::routing::get;
use axum::routing::post;
use chrono::DateTime;
use chrono::Utc;
use kernel::tasks::DelegateRequest;
use kernel::tasks::Delegation;
use kernel::tasks::DelegationError;
use serde::Serialize;
use tasks::Error;
use tasks::MeshIdentity;
use tasks::NewTask;
use tasks::RunOutcome;
use tasks::TaskOrigin;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use wire::gateway::HealthzResponse;
use wire::gateway::sessions::SessionMessage;
use wire::gateway::sessions::SessionMessageRole;
use wire::gateway::sessions::SessionPostReply;
use wire::mesh::MeshNodeStatus;
use wire::task::TaskExecutorKind;
use wire::task::TaskStatus;

#[derive(Clone)]
struct Observed {
    hits: Arc<AtomicUsize>,
    saw_authorization: Arc<AtomicBool>,
    handshake_errors: Arc<AtomicUsize>,
    connection_errors: Arc<AtomicUsize>,
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
    reason = "binding the mesh peer is fixture setup; a failure is a broken test"
)]
async fn start(material: &mesh_support::Material) -> Peer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("port").port();
    let observed = Observed {
        hits: Arc::new(AtomicUsize::new(0)),
        saw_authorization: Arc::new(AtomicBool::new(false)),
        handshake_errors: Arc::new(AtomicUsize::new(0)),
        connection_errors: Arc::new(AtomicUsize::new(0)),
    };
    let acceptor = acceptor(material);
    let router = gateway(observed.clone());
    let cancel = CancellationToken::new();
    let stop = cancel.clone();
    let task_observed = observed.clone();
    let task = tokio::spawn(async move {
        listen(listener, acceptor, router, task_observed, stop).await;
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
    reason = "the mesh peer certificate is a fixture; a failure is a broken test"
)]
fn acceptor(material: &mesh_support::Material) -> TlsAcceptor {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(material.ca.certificate.der().clone())
        .expect("mesh root");
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::clone(&provider),
    )
    .build()
    .expect("mesh verifier");
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("tls versions")
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![material.server.certificate.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(material.server.key.serialize_der()).into(),
        )
        .expect("mesh server certificate");
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    TlsAcceptor::from(Arc::new(config))
}

async fn listen(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    router: Router,
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
                let router = router.clone();
                let observed = observed.clone();
                connections.spawn(async move {
                    match acceptor.accept(stream).await {
                        Ok(tls) => serve_http(tls, router, &observed).await,
                        Err(error) => count_handshake(&observed, &error),
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

fn count_handshake(observed: &Observed, _error: &std::io::Error) {
    observed.handshake_errors.fetch_add(1, Ordering::SeqCst);
}

async fn serve_http(
    tls: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    router: Router,
    observed: &Observed,
) {
    let served = hyper::server::conn::http1::Builder::new()
        .serve_connection(
            hyper_util::rt::TokioIo::new(tls),
            hyper_util::service::TowerToHyperService::new(router),
        )
        .await;
    if let Err(error) = served {
        observed.connection_errors.fetch_add(1, Ordering::SeqCst);
        drop(error.to_string());
    }
}

fn gateway(observed: Observed) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/api/sessions/{session}/messages", post(message))
        .with_state(observed)
}

async fn health(State(observed): State<Observed>, headers: HeaderMap) -> axum::response::Response {
    note(&observed, &headers);
    json_response(&HealthzResponse {
        ok: true,
        sessions: 0,
        name: "mesh".to_owned(),
        node: "local".to_owned(),
    })
}

async fn message(
    State(observed): State<Observed>,
    headers: HeaderMap,
    Path(_session): Path<String>,
    _body: Bytes,
) -> axum::response::Response {
    note(&observed, &headers);
    json_response(&reply())
}

fn note(observed: &Observed, headers: &HeaderMap) {
    observed.hits.fetch_add(1, Ordering::SeqCst);
    if headers.contains_key(header::AUTHORIZATION) {
        observed.saw_authorization.store(true, Ordering::SeqCst);
    }
}

#[expect(
    clippy::expect_used,
    reason = "the mesh reply stamp is a fixed instant; a parse failure is a broken fixture"
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
    reason = "the mesh health document is a fixed fixture"
)]
fn json_response(body: &impl Serialize) -> axum::response::Response {
    let bytes = serde_json::to_vec(body).expect("mesh json");
    axum::response::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(bytes))
        .expect("mesh response")
}

fn mesh_task() -> NewTask {
    let mut created = common::request("ship it");
    created.executor = Some(TaskExecutorKind::Mesh);
    created.executor_target = Some("local".to_owned());
    NewTask {
        request: created,
        origin: TaskOrigin::Chat,
    }
}

#[expect(
    clippy::expect_used,
    reason = "the local base url is a fixed literal; a parse failure is a broken fixture"
)]
fn mesh_engine(
    port: u16,
    identity: Option<MeshIdentity>,
) -> tasks::Engine<testkit::TaskMemory, testkit::TaskMemory> {
    let mut directory = common::directory(url::Url::parse("http://127.0.0.1:9/").expect("url"));
    directory.mesh_node(common::node(
        "local",
        "127.0.0.1",
        port,
        &["worker"],
        MeshNodeStatus::Online,
    ));
    directory.set_mesh_identity(identity);
    let (engine, _) = common::engine_with(
        directory,
        common::settings(None, std::collections::BTreeMap::new()),
    );
    engine
}

#[expect(
    clippy::expect_used,
    reason = "a mesh run that misses its deadline or the expected outcome fails the case"
)]
async fn rejected(engine: &tasks::Engine<testkit::TaskMemory, testkit::TaskMemory>, peer: &Peer) {
    let id = engine.submit(mesh_task()).await.expect("submit");
    let error = tokio::time::timeout(Duration::from_secs(5), engine.run_one())
        .await
        .expect("deadline")
        .expect_err("peer");
    assert!(matches!(error, Error::MeshPeer(_)));
    assert_eq!(peer.observed.hits.load(Ordering::SeqCst), 0);
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Failed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mesh_executor_presents_the_node_certificate() {
    let material = mesh_support::mint("node.example.test");
    let peer = start(&material).await;
    let engine = mesh_engine(peer.port, Some(material.identity(&material.ca.pem)));
    let id = engine.submit(mesh_task()).await.expect("submit");
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
    assert!(!peer.observed.saw_authorization.load(Ordering::SeqCst));
    assert_eq!(peer.observed.handshake_errors.load(Ordering::SeqCst), 0);
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mesh_executor_without_a_certificate_does_not_dial() {
    let material = mesh_support::mint("node.example.test");
    let peer = start(&material).await;
    let engine = mesh_engine(peer.port, None);
    let error = engine.submit(mesh_task()).await.expect_err("identity");
    assert!(matches!(error, Error::MeshTlsRequired));
    assert_eq!(peer.observed.hits.load(Ordering::SeqCst), 0);
    assert_eq!(peer.observed.handshake_errors.load(Ordering::SeqCst), 0);
    assert_eq!(peer.observed.connection_errors.load(Ordering::SeqCst), 0);
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_certificate_from_another_ca_is_rejected() {
    let mesh = mesh_support::mint("node.example.test");
    let foreign = mesh_support::mint("foreign.example.test");
    let peer = start(&mesh).await;
    let engine = mesh_engine(peer.port, Some(foreign.identity(&mesh.ca.pem)));
    rejected(&engine, &peer).await;
    peer.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_foreign_ca_rejects_the_mesh_peer() {
    let mesh = mesh_support::mint("node.example.test");
    let foreign = mesh_support::mint("foreign.example.test");
    let peer = start(&mesh).await;
    let engine = mesh_engine(peer.port, Some(mesh.identity(&foreign.ca.pem)));
    rejected(&engine, &peer).await;
    peer.shutdown().await;
}

#[tokio::test]
async fn a_remote_delegation_without_a_certificate_is_rejected() {
    let mut directory = common::directory(url::Url::parse("http://127.0.0.1:9/").expect("url"));
    directory.mesh_node(common::node(
        "edge",
        "203.0.113.9",
        9,
        &["worker"],
        MeshNodeStatus::Online,
    ));
    let (engine, _) = common::engine_with(
        directory,
        common::settings(None, std::collections::BTreeMap::new()),
    );
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        engine.delegate(
            DelegateRequest {
                target: "worker@edge".to_owned(),
                prompt: "ship it".to_owned(),
                model: None,
                node: None,
                timeout: Duration::from_secs(2),
            },
            CancellationToken::new(),
        ),
    )
    .await
    .expect("deadline")
    .expect_err("rejected");
    assert!(matches!(
        error,
        DelegationError::Rejected(message)
            if message == "mesh client certificate is not configured"
    ));
}

#[test]
fn empty_mesh_material_is_refused() {
    let error = MeshIdentity::from_pem("", "key", "ca").expect_err("empty");
    assert!(matches!(error, Error::MeshTls(_)));
}

fn pem_armor(label: &str, body: &str) -> String {
    format!("-----BEGIN {label}-----\n{body}\n-----END {label}-----\n")
}

#[test]
fn invalid_mesh_material_is_refused() {
    let error = MeshIdentity::from_pem(
        &pem_armor("CERTIFICATE", "nope"),
        &pem_armor("PRIVATE KEY", "nope"),
        &pem_armor("CERTIFICATE", "nope"),
    )
    .expect_err("pem");
    assert!(matches!(error, Error::MeshTls(_)));
}

#[test]
fn a_server_certificate_still_loads_as_the_mesh_identity() {
    let material = mesh_support::mint("node.example.test");
    assert!(
        MeshIdentity::from_pem(
            &material.server.pem,
            &material.server.key_pem,
            &material.ca.pem,
        )
        .is_ok()
    );
}

#[test]
fn a_missing_mesh_file_is_unreadable() {
    let missing = std::path::Path::new("/no/such/mesh/cert.pem");
    let error = MeshIdentity::from_files(missing, missing, missing).expect_err("missing");
    assert!(matches!(error, Error::MeshTlsIo(_)));
}
