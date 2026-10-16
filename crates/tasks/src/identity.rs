use std::path::Path;

use crate::error::Error;
use crate::error::MeshMaterialError;

#[derive(Clone)]
pub struct MeshIdentity {
    certificate_pem: String,
    ca_pem: String,
    client: Option<reqwest::Identity>,
    cas: Vec<reqwest::Certificate>,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "Debug must not print the mesh client key or certificate PEM"
    )
)]
impl std::fmt::Debug for MeshIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MeshIdentity")
            .field("has_client_certificate", &self.client.is_some())
            .field("ca_certificates", &self.cas.len())
            .finish_non_exhaustive()
    }
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "equality is the certificate and CA PEM; reqwest Identity holds the private key and is not Eq"
    )
)]
impl PartialEq for MeshIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.certificate_pem == other.certificate_pem && self.ca_pem == other.ca_pem
    }
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "equality is the certificate and CA PEM; reqwest Identity holds the private key and is not Eq"
    )
)]
impl Eq for MeshIdentity {}

impl MeshIdentity {
    pub fn from_pem(
        certificate_pem: &str,
        private_key_pem: &str,
        ca_pem: &str,
    ) -> Result<Self, Error> {
        if certificate_pem.is_empty() || private_key_pem.is_empty() {
            return Err(Error::MeshTls(MeshMaterialError::Empty));
        }
        let client =
            reqwest::Identity::from_pem(format!("{certificate_pem}\n{private_key_pem}").as_bytes())
                .map_err(|source| Error::MeshTls(MeshMaterialError::Invalid(Box::new(source))))?;
        Self::assemble(certificate_pem.to_owned(), ca_pem, Some(client))
    }

    pub fn from_ca(ca_pem: &str) -> Result<Self, Error> {
        Self::assemble(String::new(), ca_pem, None)
    }

    pub fn from_files(cert: &Path, key: &Path, ca: &Path) -> Result<Self, Error> {
        let certificate_pem = read_pem(cert)?;
        let private_key_pem = read_pem(key)?;
        let ca_pem = read_pem(ca)?;
        Self::from_pem(&certificate_pem, &private_key_pem, &ca_pem)
    }

    #[must_use]
    pub fn presents_certificate(&self) -> bool {
        self.client.is_some()
    }

    pub(crate) fn presented(&self) -> Option<reqwest::Identity> {
        self.client.clone()
    }

    pub(crate) fn trust(&self) -> Vec<reqwest::Certificate> {
        self.cas.clone()
    }

    fn assemble(
        certificate_pem: String,
        ca_pem: &str,
        client: Option<reqwest::Identity>,
    ) -> Result<Self, Error> {
        let cas = trust_anchors(ca_pem)?;
        let mut builder = reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .tls_certs_only(cas.clone());
        if let Some(identity) = client.clone() {
            builder = builder.identity(identity);
        }
        builder
            .build()
            .map_err(|source| Error::MeshTls(MeshMaterialError::Invalid(Box::new(source))))?;
        Ok(Self {
            certificate_pem,
            ca_pem: ca_pem.to_owned(),
            client,
            cas,
        })
    }
}

fn trust_anchors(ca_pem: &str) -> Result<Vec<reqwest::Certificate>, Error> {
    if ca_pem.is_empty() {
        return Err(Error::MeshTls(MeshMaterialError::Empty));
    }
    let cas = reqwest::Certificate::from_pem_bundle(ca_pem.as_bytes())
        .map_err(|source| Error::MeshTls(MeshMaterialError::Invalid(Box::new(source))))?;
    if cas.is_empty() {
        return Err(Error::MeshTls(MeshMaterialError::Empty));
    }
    Ok(cas)
}

fn read_pem(path: &Path) -> Result<String, Error> {
    if path.as_os_str().is_empty() {
        return Err(Error::MeshTlsRequired);
    }
    std::fs::read_to_string(path).map_err(Error::MeshTlsIo)
}
