use std::collections::VecDeque;
use std::pin::pin;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::Extension;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use parking_lot::Mutex;
use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::DistinguishedName;
use rcgen::DnType;
use rcgen::ExtendedKeyUsagePurpose;
use rcgen::IsCa;
use rcgen::Issuer;
use rcgen::KeyPair;
use rcgen::KeyUsagePurpose;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

use crate::Holder;
use crate::document::RevocationCause;

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(crate) struct Minted {
    pub(crate) holder: Holder,
    pub(crate) der: Vec<u8>,
}

pub(crate) struct Issued {
    pub(crate) leaf: String,
    pub(crate) intermediate: String,
}

pub(crate) enum Peer {
    Absent,
    Der(Vec<u8>),
}

pub(crate) enum ExpectedBody {
    Empty,
    Sign {
        csr: String,
        ott: String,
    },
    Revoke {
        serial: String,
        reason_code: u8,
        reason: RevocationCause,
        passive: bool,
    },
}

pub(crate) enum Reply {
    Document {
        status: u16,
        content_type: &'static str,
        body: Vec<u8>,
    },
    Redirect {
        status: u16,
        location: String,
    },
    Hold,
}

pub(crate) struct Step {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) query: Option<String>,
    pub(crate) peer: Peer,
    pub(crate) body: ExpectedBody,
    pub(crate) reply: Reply,
}

struct Shared {
    steps: Mutex<VecDeque<Step>>,
    mismatch: Mutex<Option<String>>,
    accepted: Mutex<usize>,
    arrived: Notify,
    connection: Mutex<Option<String>>,
}

struct CaRoot {
    params: CertificateParams,
    key: KeyPair,
    certificate: rcgen::Certificate,
}

struct Prepared {
    tls_pem: String,
    root_params: CertificateParams,
    root_key: KeyPair,
    acceptor: TlsAcceptor,
}

pub(crate) struct Server {
    url: url::Url,
    tls_pem: String,
    root_params: CertificateParams,
    root_key: KeyPair,
    shared: Arc<Shared>,
    cancel: CancellationToken,
    task: Option<JoinHandle<TestResult>>,
}

impl Server {
    pub(crate) async fn start() -> TestResult<Self> {
        let prepared = prepare()?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("https://{}/", listener.local_addr()?).parse()?;
        let shared = Arc::new(Shared {
            steps: Mutex::new(VecDeque::new()),
            mismatch: Mutex::new(None),
            accepted: Mutex::new(0),
            arrived: Notify::new(),
            connection: Mutex::new(None),
        });
        let cancel = CancellationToken::new();
        let task = spawn_accept(
            listener,
            prepared.acceptor,
            Arc::clone(&shared),
            cancel.clone(),
        );
        Ok(Self {
            url,
            tls_pem: prepared.tls_pem,
            root_params: prepared.root_params,
            root_key: prepared.root_key,
            shared,
            cancel,
            task: Some(task),
        })
    }

    pub(crate) fn url(&self) -> &url::Url {
        &self.url
    }

    pub(crate) fn tls_pem(&self) -> &str {
        &self.tls_pem
    }

    pub(crate) fn push(&self, step: Step) {
        self.shared.steps.lock().push_back(step);
    }

    pub(crate) fn mismatch(&self) -> Option<String> {
        self.shared.mismatch.lock().clone()
    }

    pub(crate) fn remaining(&self) -> usize {
        self.shared.steps.lock().len()
    }

    pub(crate) fn accepted(&self) -> usize {
        *self.shared.accepted.lock()
    }

    pub(crate) async fn until_accepted(&self) {
        loop {
            let notified = pin!(self.shared.arrived.notified());
            if self.accepted() > 0 {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn mint(&self, name: &str) -> TestResult<Minted> {
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(vec![format!("{name}.example.test")])?;
        leaf_uses(&mut params);
        let certificate = params.signed_by(
            &key,
            &Issuer::from_params(&self.root_params, &self.root_key),
        )?;
        let holder = Holder::from_pem(&certificate.pem(), &key.serialize_pem())?;
        Ok(Minted {
            holder,
            der: certificate.der().to_vec(),
        })
    }

    pub(crate) fn issue(&self) -> TestResult<Issued> {
        let intermediate_key = KeyPair::generate()?;
        let mut intermediate_params = CertificateParams::default();
        window(&mut intermediate_params);
        intermediate_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        intermediate_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, "stepca test intermediate");
        intermediate_params.distinguished_name = name;
        let intermediate = intermediate_params.signed_by(
            &intermediate_key,
            &Issuer::from_params(&self.root_params, &self.root_key),
        )?;
        let leaf_key = KeyPair::generate()?;
        let mut leaf_params = CertificateParams::new(vec!["leaf.example.test".to_owned()])?;
        leaf_uses(&mut leaf_params);
        let leaf = leaf_params.signed_by(
            &leaf_key,
            &Issuer::from_params(&intermediate_params, &intermediate_key),
        )?;
        Ok(Issued {
            leaf: leaf.pem(),
            intermediate: intermediate.pem(),
        })
    }

    pub(crate) async fn close(self) -> TestResult {
        let mismatch = self.mismatch();
        let remaining = self.remaining();
        let connection = self.shared.connection.lock().clone();
        let stopped = self.stop().await;
        if let Some(mismatch) = mismatch {
            stopped?;
            return Err(mismatch.into());
        }
        stopped?;
        if remaining == 0 {
            Ok(())
        } else {
            let detail = match connection {
                Some(error) => error,
                None => "request did not arrive".to_owned(),
            };
            Err(format!("{remaining} steps left: {detail}").into())
        }
    }

    pub(crate) async fn stop(mut self) -> TestResult {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            tokio::time::timeout(Duration::from_secs(5), task).await???;
        }
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn prepare() -> TestResult<Prepared> {
    let root = ca_root()?;
    let (server_cert, server_key) = server_certificate(&root)?;
    let acceptor = acceptor(&root.certificate, &server_cert, &server_key)?;
    Ok(Prepared {
        tls_pem: root.certificate.pem(),
        root_params: root.params,
        root_key: root.key,
        acceptor,
    })
}

fn ca_root() -> TestResult<CaRoot> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(vec!["stepca.example.test".to_owned()])?;
    window(&mut params);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "stepca test root");
    params.distinguished_name = name;
    let certificate = params.self_signed(&key)?;
    Ok(CaRoot {
        params,
        key,
        certificate,
    })
}

fn server_certificate(root: &CaRoot) -> TestResult<(rcgen::Certificate, KeyPair)> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])?;
    window(&mut params);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    let certificate = params.signed_by(&key, &Issuer::from_params(&root.params, &root.key))?;
    Ok((certificate, key))
}

fn window(params: &mut CertificateParams) {
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    params.not_after = rcgen::date_time_ymd(2027, 1, 1);
}

fn leaf_uses(params: &mut CertificateParams) {
    window(params);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
}

fn acceptor(
    root: &rcgen::Certificate,
    server_cert: &rcgen::Certificate,
    server_key: &KeyPair,
) -> TestResult<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.der().clone())?;
    let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::clone(&provider),
    )
    .allow_unauthenticated()
    .build()?;
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server_cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
        )?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn spawn_accept(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    shared: Arc<Shared>,
    cancel: CancellationToken,
) -> JoinHandle<TestResult> {
    tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::<TestResult>::new();
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let acceptor = acceptor.clone();
                    let shared = Arc::clone(&shared);
                    let stop = cancel.clone();
                    connections.spawn(async move {
                        tokio::select! {
                            biased;
                            () = stop.cancelled() => Ok(()),
                            result = serve_one(acceptor, stream, Arc::clone(&shared)) => {
                                if let Err(error) = &result {
                                    remember_connection(&shared, &error.to_string());
                                }
                                Ok(())
                            }
                        }
                    });
                }
                joined = connections.join_next(), if !connections.is_empty() => {
                    joined.ok_or("missing connection")???;
                }
            }
        }
        while let Some(joined) = connections.join_next().await {
            joined??;
        }
        Ok(())
    })
}

async fn serve_one(
    acceptor: TlsAcceptor,
    stream: tokio::net::TcpStream,
    shared: Arc<Shared>,
) -> TestResult {
    let tls = acceptor.accept(stream).await?;
    let peer = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|cert| cert.as_ref().to_vec());
    let router = Router::new()
        .fallback(handle)
        .layer(Extension(peer))
        .with_state(shared);
    hyper::server::conn::http1::Builder::new()
        .serve_connection(
            hyper_util::rt::TokioIo::new(tls),
            hyper_util::service::TowerToHyperService::new(router),
        )
        .await?;
    Ok(())
}

async fn handle(
    State(shared): State<Arc<Shared>>,
    Extension(peer): Extension<Option<Vec<u8>>>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match axum::body::to_bytes(body, 2 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(error) => return text(StatusCode::BAD_REQUEST, error.to_string()),
    };
    let Some(step) = shared.steps.lock().pop_front() else {
        remember(&shared, "unexpected request with no scripted step");
        return text(StatusCode::INTERNAL_SERVER_ERROR, "no step".to_owned());
    };
    if let Err(error) = matches_step(&step, &parts, &bytes, peer.as_deref()) {
        let message = error.to_string();
        remember(&shared, &message);
        return text(StatusCode::INTERNAL_SERVER_ERROR, message);
    }
    if matches!(step.reply, Reply::Hold) {
        note_accepted(&shared);
        std::future::pending::<()>().await;
    }
    reply(step.reply)
}

fn matches_step(
    step: &Step,
    parts: &axum::http::request::Parts,
    body: &[u8],
    peer: Option<&[u8]>,
) -> TestResult {
    if parts.method.as_str() != step.method {
        return Err(format!("method {} wanted {}", parts.method, step.method).into());
    }
    if parts.uri.path() != step.path {
        return Err(format!("path {} wanted {}", parts.uri.path(), step.path).into());
    }
    if parts.uri.query() != step.query.as_deref() {
        return Err(format!("query {:?} wanted {:?}", parts.uri.query(), step.query).into());
    }
    match_peer(&step.peer, peer)?;
    match_body(&step.body, body)
}

fn match_peer(expected: &Peer, peer: Option<&[u8]>) -> TestResult {
    match (expected, peer) {
        (Peer::Absent, None) => Ok(()),
        (Peer::Der(expected), Some(actual)) if expected.as_slice() == actual => Ok(()),
        (Peer::Absent, Some(_)) => Err("client certificate was not expected".into()),
        (Peer::Der(_), None) => Err("client certificate was required".into()),
        (Peer::Der(_), Some(_)) => Err("client certificate did not match".into()),
    }
}

fn match_body(expected: &ExpectedBody, body: &[u8]) -> TestResult {
    match expected {
        ExpectedBody::Empty => {
            if body.is_empty() {
                Ok(())
            } else {
                Err("expected an empty body".into())
            }
        }
        ExpectedBody::Sign { csr, ott } => {
            let parsed: SignForm = serde_json::from_slice(body)?;
            if parsed.csr == *csr && parsed.ott == *ott {
                Ok(())
            } else {
                Err("sign body did not match".into())
            }
        }
        ExpectedBody::Revoke {
            serial,
            reason_code,
            reason,
            passive,
        } => {
            let parsed: RevokeForm = serde_json::from_slice(body)?;
            if parsed.serial == *serial
                && parsed.reason_code == *reason_code
                && parsed.reason == *reason
                && parsed.passive == *passive
            {
                Ok(())
            } else {
                Err("revoke body did not match".into())
            }
        }
    }
}

fn reply(reply: Reply) -> Response {
    match reply {
        Reply::Hold => text(StatusCode::SERVICE_UNAVAILABLE, "held".to_owned()),
        Reply::Redirect { status, location } => {
            (status_code(status), [("location", location)]).into_response()
        }
        Reply::Document {
            status,
            content_type,
            body,
        } => (status_code(status), [("content-type", content_type)], body).into_response(),
    }
}

fn status_code(code: u16) -> StatusCode {
    match code {
        200 => StatusCode::OK,
        201 => StatusCode::CREATED,
        204 => StatusCode::NO_CONTENT,
        302 => StatusCode::FOUND,
        307 => StatusCode::TEMPORARY_REDIRECT,
        308 => StatusCode::PERMANENT_REDIRECT,
        400 => StatusCode::BAD_REQUEST,
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        503 => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn note_accepted(shared: &Shared) {
    *shared.accepted.lock() += 1;
    shared.arrived.notify_one();
}

fn remember(shared: &Shared, error: &str) {
    let mut slot = shared.mismatch.lock();
    if slot.is_none() {
        *slot = Some(error.to_owned());
    }
}

fn remember_connection(shared: &Shared, error: &str) {
    let mut slot = shared.connection.lock();
    if slot.is_none() {
        *slot = Some(error.to_owned());
    }
}

fn text(status: StatusCode, message: String) -> Response {
    (status, message).into_response()
}

#[derive(Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SignForm {
    csr: String,
    ott: String,
}

#[derive(Debug, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RevokeForm {
    serial: String,
    reason_code: u8,
    reason: RevocationCause,
    passive: bool,
}
