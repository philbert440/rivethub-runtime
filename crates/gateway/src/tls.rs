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

use crate::server::BindError;
use crate::server::BindSource;
use crate::server::is_bind_loopback;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsPaths {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub ca: Option<PathBuf>,
}

#[derive(Debug, Error)]
pub enum TlsError {
    #[error("failed to read PEM from {}", path.display())]
    Pem {
        path: PathBuf,
        #[source]
        source: rustls::pki_types::pem::Error,
    },
    #[error("rustls server configuration failed")]
    Rustls(#[source] rustls::Error),
    #[error("client certificate verifier could not be built")]
    Verifier(#[source] rustls::server::VerifierBuilderError),
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

pub(crate) fn tls_config(paths: &TlsPaths) -> Result<ServerConfig, TlsError> {
    let certs = load_certs(&paths.cert)?;
    let key = load_key(&paths.key)?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let builder = ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(TlsError::Rustls)?;
    let with_certs = match &paths.ca {
        Some(ca) => {
            let mut roots = RootCertStore::empty();
            for cert in load_certs(ca)? {
                roots.add(cert).map_err(TlsError::Rustls)?;
            }
            let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .build()
                .map_err(TlsError::Verifier)?;
            builder.with_client_cert_verifier(verifier)
        }
        None => builder.with_no_client_auth(),
    };
    with_certs
        .with_single_cert(certs, key)
        .map_err(TlsError::Rustls)
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    CertificateDer::pem_file_iter(path)
        .map_err(|source| TlsError::Pem {
            path: path.to_path_buf(),
            source,
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|source| TlsError::Pem {
            path: path.to_path_buf(),
            source,
        })
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsError> {
    PrivateKeyDer::from_pem_file(path).map_err(|source| TlsError::Pem {
        path: path.to_path_buf(),
        source,
    })
}
