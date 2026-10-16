use std::path::Path;

use crate::error::Error;
use crate::error::MeshMaterialError;

#[derive(Clone)]
pub struct MeshIdentity {
    certificate_pem: String,
    ca_pem: String,
    client: reqwest::Identity,
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
            .field("has_client_certificate", &true)
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
        if certificate_pem.is_empty() || private_key_pem.is_empty() || ca_pem.is_empty() {
            return Err(Error::MeshTls(MeshMaterialError::Empty));
        }
        let cas = reqwest::Certificate::from_pem_bundle(ca_pem.as_bytes())
            .map_err(|source| Error::MeshTls(MeshMaterialError::Invalid(Box::new(source))))?;
        if cas.is_empty() {
            return Err(Error::MeshTls(MeshMaterialError::Empty));
        }
        let client =
            reqwest::Identity::from_pem(format!("{certificate_pem}\n{private_key_pem}").as_bytes())
                .map_err(|source| Error::MeshTls(MeshMaterialError::Invalid(Box::new(source))))?;
        reqwest::Client::builder()
            .https_only(true)
            .no_proxy()
            .tls_certs_only(cas.clone())
            .identity(client.clone())
            .build()
            .map_err(|source| Error::MeshTls(MeshMaterialError::Invalid(Box::new(source))))?;
        Ok(Self {
            certificate_pem: certificate_pem.to_owned(),
            ca_pem: ca_pem.to_owned(),
            client,
            cas,
        })
    }

    pub fn from_files(cert: &Path, key: &Path, ca: &Path) -> Result<Self, Error> {
        let certificate_pem = read_pem(cert)?;
        let private_key_pem = read_pem(key)?;
        let ca_pem = read_pem(ca)?;
        Self::from_pem(&certificate_pem, &private_key_pem, &ca_pem)
    }

    pub(crate) fn client(&self) -> reqwest::Identity {
        self.client.clone()
    }

    pub(crate) fn trust(&self) -> Vec<reqwest::Certificate> {
        self.cas.clone()
    }
}

fn read_pem(path: &Path) -> Result<String, Error> {
    if path.as_os_str().is_empty() {
        return Err(Error::MeshTlsRequired);
    }
    std::fs::read_to_string(path).map_err(Error::MeshTlsIo)
}
