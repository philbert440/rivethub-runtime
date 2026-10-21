use std::net::Ipv4Addr;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::Response;
use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::DistinguishedName;
use rcgen::DnType;
use rcgen::ExtendedKeyUsagePurpose;
use rcgen::IsCa;
use rcgen::Issuer;
use rcgen::KeyPair;
use rcgen::KeyUsagePurpose;
use rcgen::SanType;
use rustls::ServerConfig;
use tokio::net::TcpListener;
use tokio::net::TcpStream;
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::Result;

#[derive(Debug, serde::Deserialize)]
struct Catalog {
    #[serde(rename = "crlPem")]
    crl_pem: String,
}

#[derive(Debug, serde::Serialize)]
struct RootsDocument {
    crts: Vec<String>,
}

struct Shared {
    root_pem: String,
    crl_pem: String,
}

pub(crate) struct Replay {
    pub(crate) url: Url,
    pub(crate) root_pem: PathBuf,
    pub(crate) crl_pem: String,
    _directory: tempfile::TempDir,
    cancel: CancellationToken,
    task: Option<tokio::task::JoinHandle<()>>,
    failure: Arc<parking_lot::Mutex<Option<String>>>,
}

impl Replay {
    pub(crate) async fn start() -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let prepared = prepare()?;
        let root_pem = directory.path().join("root.pem");
        std::fs::write(&root_pem, &prepared.root_pem)?;
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
        let port = listener.local_addr()?.port();
        let url = Url::parse(&format!("https://127.0.0.1:{port}"))?;
        let failure = Arc::new(parking_lot::Mutex::new(None));
        let shared = Arc::new(Shared {
            root_pem: prepared.root_pem,
            crl_pem: prepared.crl_pem.clone(),
        });
        let cancel = CancellationToken::new();
        let task = spawn_accept(
            listener,
            prepared.acceptor,
            shared,
            Arc::clone(&failure),
            cancel.clone(),
        );
        Ok(Self {
            url,
            root_pem,
            crl_pem: prepared.crl_pem,
            _directory: directory,
            cancel,
            task: Some(task),
            failure,
        })
    }

    pub(crate) fn failure(&self) -> Option<String> {
        self.failure.lock().clone()
    }
}

impl Drop for Replay {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

struct Prepared {
    root_pem: String,
    crl_pem: String,
    acceptor: TlsAcceptor,
}

fn prepare() -> Result<Prepared> {
    let root_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut root_params = CertificateParams::default();
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    root_params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    root_params.not_after = rcgen::date_time_ymd(2027, 1, 1);
    let mut root_name = DistinguishedName::new();
    root_name.push(DnType::CommonName, "Example Test Root");
    root_params.distinguished_name = root_name;
    let root = root_params.self_signed(&root_key)?;
    let server_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut server_params = CertificateParams::default();
    server_params.subject_alt_names = vec![SanType::IpAddress(std::net::IpAddr::V4(
        Ipv4Addr::LOCALHOST,
    ))];
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    server_params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    server_params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    server_params.not_after = rcgen::date_time_ymd(2027, 1, 1);
    let server =
        server_params.signed_by(&server_key, &Issuer::from_params(&root_params, &root_key))?;
    let catalog: Catalog =
        serde_json::from_slice(&std::fs::read(testkit::recording("stepca/api.json"))?)?;
    Ok(Prepared {
        root_pem: root.pem(),
        crl_pem: catalog.crl_pem,
        acceptor: acceptor(&server, &server_key)?,
    })
}

fn acceptor(server: &rcgen::Certificate, server_key: &KeyPair) -> Result<TlsAcceptor> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(
            vec![server.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
        )?;
    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn spawn_accept(
    listener: TcpListener,
    acceptor: TlsAcceptor,
    state: Arc<Shared>,
    failure: Arc<parking_lot::Mutex<Option<String>>>,
    cancel: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                accepted = listener.accept() => {
                    match accepted {
                        Ok((stream, _)) => {
                            let acceptor = acceptor.clone();
                            let state = Arc::clone(&state);
                            let failure = Arc::clone(&failure);
                            tokio::spawn(async move {
                                match serve_one(acceptor, stream, state).await {
                                    Ok(()) => {}
                                    Err(error) => remember(&failure, error),
                                }
                            });
                        }
                        Err(error) => {
                            remember(&failure, error);
                            break;
                        }
                    }
                }
            }
        }
    })
}

async fn serve_one(acceptor: TlsAcceptor, stream: TcpStream, state: Arc<Shared>) -> Result {
    let tls = acceptor.accept(stream).await?;
    let router = Router::new().fallback(handle).with_state(state);
    hyper::server::conn::http1::Builder::new()
        .serve_connection(
            hyper_util::rt::TokioIo::new(tls),
            hyper_util::service::TowerToHyperService::new(router),
        )
        .await?;
    Ok(())
}

async fn handle(State(state): State<Arc<Shared>>, request: Request) -> Response {
    if request.method() == axum::http::Method::GET && request.uri().path() == "/roots" {
        roots(&state.root_pem)
    } else if request.method() == axum::http::Method::GET
        && request.uri().path() == "/1.0/crl"
        && request.uri().query() == Some("pem=true")
    {
        (
            StatusCode::OK,
            [("content-type", "application/x-pem-file")],
            state.crl_pem.clone(),
        )
            .into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

fn roots(pem: &str) -> Response {
    match serde_json::to_vec(&RootsDocument {
        crts: vec![pem.to_owned()],
    }) {
        Ok(body) => (
            StatusCode::CREATED,
            [("content-type", "application/json")],
            body,
        )
            .into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

fn remember(slot: &parking_lot::Mutex<Option<String>>, error: impl std::fmt::Display) {
    let mut failure = slot.lock();
    if failure.is_none() {
        *failure = Some(error.to_string());
    }
}
