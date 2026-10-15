use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use parking_lot::Mutex;
use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::ExtendedKeyUsagePurpose;
use rcgen::IsCa;
use rcgen::Issuer;
use rcgen::KeyPair;
use rcgen::KeyUsagePurpose;
use rcgen::SanType;
use rustls::pki_types::PrivatePkcs8KeyDer;
use serde::Deserialize;
use serde::Serialize;
use stepca::CipherSuite;
use stepca::TlsOptions;
use stepca::TlsVersion;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use url::Url;

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub(crate) struct Refusal {
    pub(crate) token: String,
    pub(crate) status: u16,
    pub(crate) message: String,
}

#[derive(Debug)]
pub(crate) struct Stub {
    pub(crate) url: Url,
    pub(crate) root_pem: String,
    pub(crate) root_path: std::path::PathBuf,
    pub(crate) password_path: std::path::PathBuf,
    seen: Arc<Mutex<Vec<String>>>,
    cancel: CancellationToken,
    task: Option<JoinHandle<TestResult>>,
    _directory: tempfile::TempDir,
}

impl Stub {
    pub(crate) fn saw(&self, token: &str) -> bool {
        self.seen.lock().iter().any(|seen| seen == token)
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub(crate) async fn start(crl_pem: String, refusals: Vec<Refusal>) -> TestResult<Stub> {
    let minted = mint()?;
    let directory = tempfile::tempdir()?;
    let root_path = directory.path().join("root.pem");
    let password_path = directory.path().join("provisioner.password");
    std::fs::write(&root_path, &minted.root_pem)?;
    std::fs::write(&password_path, "secret\n")?;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let shared = Arc::new(Shared {
        root_pem: minted.root_pem.clone(),
        intermediate_pem: minted.intermediate_pem,
        intermediate_key: minted.intermediate_key,
        crl_pem,
        refusals,
        seen: Arc::clone(&seen),
    });
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let acceptor = acceptor(&minted.server_der, &minted.server_key)?;
    let cancel = CancellationToken::new();
    let task = spawn_accept(listener, acceptor, shared, cancel.clone());
    Ok(Stub {
        url: Url::parse(&format!("https://127.0.0.1:{port}/"))?,
        root_pem: minted.root_pem,
        root_path,
        password_path,
        seen,
        cancel,
        task: Some(task),
        _directory: directory,
    })
}

struct Minted {
    root_pem: String,
    intermediate_pem: String,
    intermediate_key: String,
    server_der: rustls::pki_types::CertificateDer<'static>,
    server_key: Vec<u8>,
}

struct Shared {
    root_pem: String,
    intermediate_pem: String,
    intermediate_key: String,
    crl_pem: String,
    refusals: Vec<Refusal>,
    seen: Arc<Mutex<Vec<String>>>,
}

fn mint() -> TestResult<Minted> {
    let root_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut root_params = CertificateParams::new(vec!["RivetHub Test Root".to_owned()])?;
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    root_params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    root_params.not_after = rcgen::date_time_ymd(2036, 1, 1);
    let root = root_params.self_signed(&root_key)?;
    let root_issuer = Issuer::from_params(&root_params, &root_key);
    let intermediate_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut intermediate_params =
        CertificateParams::new(vec!["RivetHub Test Intermediate".to_owned()])?;
    intermediate_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    intermediate_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    intermediate_params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    intermediate_params.not_after = rcgen::date_time_ymd(2035, 1, 1);
    let intermediate = intermediate_params.signed_by(&intermediate_key, &root_issuer)?;
    let server_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut server_params =
        CertificateParams::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])?;
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    server_params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    server_params.not_after = rcgen::date_time_ymd(2027, 1, 1);
    let server = server_params.signed_by(&server_key, &root_issuer)?;
    Ok(Minted {
        root_pem: root.pem(),
        intermediate_pem: intermediate.pem(),
        intermediate_key: intermediate_key.serialize_pem(),
        server_der: server.der().clone(),
        server_key: server_key.serialize_der(),
    })
}

fn acceptor(
    server: &rustls::pki_types::CertificateDer<'static>,
    key: &[u8],
) -> TestResult<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![server.clone()],
            PrivatePkcs8KeyDer::from(key.to_vec()).into(),
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
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(()),
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let acceptor = acceptor.clone();
                    let shared = Arc::clone(&shared);
                    tokio::spawn(async move {
                        if let Ok(tls) = acceptor.accept(stream).await {
                            let router = Router::new().fallback(handle).with_state(shared);
                            drop(
                                hyper::server::conn::http1::Builder::new()
                                    .serve_connection(
                                        hyper_util::rt::TokioIo::new(tls),
                                        hyper_util::service::TowerToHyperService::new(router),
                                    )
                                    .await,
                            );
                        }
                    });
                }
            }
        }
    })
}

async fn handle(State(shared): State<Arc<Shared>>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let query = request.uri().query().map(str::to_owned);
    let method = request.method().clone();
    let bytes = match axum::body::to_bytes(request.into_body(), 2 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    if method == axum::http::Method::GET && path == "/roots" {
        return json_response(
            StatusCode::CREATED,
            &Roots {
                crts: vec![shared.root_pem.clone()],
            },
        );
    }
    if method == axum::http::Method::GET
        && path == "/1.0/crl"
        && query.as_deref() == Some("pem=true")
    {
        return (StatusCode::OK, shared.crl_pem.clone()).into_response();
    }
    if method == axum::http::Method::POST && path == "/1.0/sign" {
        return sign(&shared, &bytes);
    }
    (StatusCode::NOT_FOUND, "unexpected step-ca request").into_response()
}

fn sign(shared: &Shared, bytes: &[u8]) -> Response {
    let form = match serde_json::from_slice::<SignForm>(bytes) {
        Ok(form) => form,
        Err(error) => return (StatusCode::BAD_REQUEST, error.to_string()).into_response(),
    };
    shared.seen.lock().push(form.ott.clone());
    if let Some(refusal) = shared.refusals.iter().find(|item| item.token == form.ott) {
        return json_response(
            status_code(refusal.status),
            &Failure {
                status: refusal.status,
                message: refusal.message.clone(),
            },
        );
    }
    match issued(shared) {
        Ok(document) => json_response(StatusCode::CREATED, &document),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

fn issued(shared: &Shared) -> TestResult<SignDocument> {
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut params = CertificateParams::default();
    params.subject_alt_names = vec![SanType::URI(
        "spiffe://rivet.mesh/node/rec-node".try_into()?,
    )];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    params.not_after = rcgen::date_time_ymd(2027, 1, 1);
    let issuer = Issuer::from_ca_cert_pem(
        &shared.intermediate_pem,
        KeyPair::from_pem(&shared.intermediate_key)?,
    )?;
    let leaf = params.signed_by(&key, &issuer)?;
    let leaf_pem = leaf.pem();
    Ok(SignDocument {
        crt: leaf_pem.clone(),
        ca: shared.intermediate_pem.clone(),
        cert_chain: vec![leaf_pem, shared.intermediate_pem.clone()],
        tls_options: TlsOptions {
            cipher_suites: vec![CipherSuite::Chacha20Poly1305, CipherSuite::Aes128Gcm],
            min_version: TlsVersion::V12,
            max_version: TlsVersion::V13,
            renegotiation: false,
        },
    })
}

fn json_response(status: StatusCode, body: &impl Serialize) -> Response {
    match serde_json::to_vec(body) {
        Ok(bytes) => (status, [("content-type", "application/json")], bytes).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

fn status_code(code: u16) -> StatusCode {
    match code {
        400 => StatusCode::BAD_REQUEST,
        401 => StatusCode::UNAUTHORIZED,
        403 => StatusCode::FORBIDDEN,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Debug, Deserialize)]
struct SignForm {
    ott: String,
}

#[derive(Debug, Serialize)]
struct Failure {
    status: u16,
    message: String,
}

#[derive(Debug, Serialize)]
struct Roots {
    crts: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignDocument {
    crt: String,
    ca: String,
    cert_chain: Vec<String>,
    tls_options: TlsOptions,
}
