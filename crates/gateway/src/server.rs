use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use rustls::RootCertStore;
use rustls::ServerConfig;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::PrivateKeyDer;
use rustls::pki_types::pem::PemObject;
use rustls::server::WebPkiClientVerifier;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_util::sync::CancellationToken;

use crate::Gateway;

use axum::serve::ListenerExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindSource {
    pub host: String,
    pub port: u16,
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub tls_ca: Option<PathBuf>,
}

impl BindSource {
    #[must_use]
    pub fn from_config(config: &config::Config) -> Self {
        match &config.gateway {
            Some(section) => Self {
                host: section
                    .host
                    .clone()
                    .unwrap_or_else(|| "127.0.0.1".to_string()),
                port: section.port,
                tls_cert: section.tls_cert.clone().map(PathBuf::from),
                tls_key: section.tls_key.clone().map(PathBuf::from),
                tls_ca: None,
            },
            None => Self {
                host: "127.0.0.1".to_string(),
                port: 5174,
                tls_cert: None,
                tls_key: None,
                tls_ca: None,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub ca: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum BindError {
    #[error(
        "[den-server] refusing to bind {host}: gateway TLS is required off-loopback (set RIVETOS_DEN_TLS_CERT + RIVETOS_DEN_TLS_KEY to the node cert from rivet-ca issue-node; enroll clients with rivet-ca issue-client). Or bind den.host to 127.0.0.1."
    )]
    TlsRequired { host: String },
    #[error(
        "[den-server] TLS CA chain missing at {} — cannot verify device client certs",
        path.display()
    )]
    CaMissing { path: PathBuf },
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[must_use]
pub fn is_bind_loopback(host: &str) -> bool {
    matches!(
        host.trim().to_ascii_lowercase().as_str(),
        "127.0.0.1" | "::1" | "localhost" | "::ffff:127.0.0.1"
    )
}

fn non_empty(path: Option<&PathBuf>) -> Option<&Path> {
    path.filter(|path| !path.as_os_str().is_empty())
        .map(Path::new)
}

pub fn resolve_tls(source: &BindSource) -> Result<Option<TlsPaths>, BindError> {
    let (Some(cert), Some(key)) = (
        non_empty(source.tls_cert.as_ref()),
        non_empty(source.tls_key.as_ref()),
    ) else {
        if is_bind_loopback(&source.host) {
            return Ok(None);
        }
        return Err(BindError::TlsRequired {
            host: source.host.clone(),
        });
    };
    std::fs::metadata(cert)?;
    std::fs::metadata(key)?;
    if let Some(ca) = &source.tls_ca
        && !ca.is_file()
    {
        return Err(BindError::CaMissing { path: ca.clone() });
    }
    Ok(Some(TlsPaths {
        cert: cert.to_path_buf(),
        key: key.to_path_buf(),
        ca: source.tls_ca.clone(),
    }))
}

pub async fn bind(source: &BindSource) -> Result<TcpListener, BindError> {
    Ok(TcpListener::bind((source.host.as_str(), source.port)).await?)
}

pub async fn serve(
    listener: TcpListener,
    gateway: Arc<Gateway>,
    tls: Option<TlsPaths>,
    shutdown: CancellationToken,
) -> io::Result<()> {
    let app = gateway.router(tls.is_some());
    match tls {
        Some(paths) => {
            let config = Arc::new(tls_config(&paths)?);
            let listener = TlsListener {
                inner: listener,
                acceptor: TlsAcceptor::from(config),
            }
            .tap_io(|_| {});
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
        }
        None => {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
        }
    }
}

struct TlsListener {
    inner: TcpListener,
    acceptor: TlsAcceptor,
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<tokio::net::TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (stream, addr) =
                <TcpListener as axum::serve::Listener>::accept(&mut self.inner).await;
            match self.acceptor.accept(stream).await {
                Ok(tls) => return (tls, addr),
                Err(error) => {
                    tracing::warn!("tls handshake from {addr} failed: {error}");
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

fn tls_config(paths: &TlsPaths) -> io::Result<ServerConfig> {
    let certs = load_certs(&paths.cert)?;
    let key = load_key(&paths.key)?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let with_certs = match &paths.ca {
        Some(ca) => {
            let mut roots = RootCertStore::empty();
            for cert in load_certs(ca)? {
                roots
                    .add(cert)
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            }
            let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            builder.with_client_cert_verifier(verifier)
        }
        None => builder.with_no_client_auth(),
    };
    with_certs
        .with_single_cert(certs, key)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn load_certs(path: &Path) -> io::Result<Vec<CertificateDer<'static>>> {
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(certs)
}

fn load_key(path: &Path) -> io::Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_file(path)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}
