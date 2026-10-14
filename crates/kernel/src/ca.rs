use crate::BoxError;
use async_trait::async_trait;

#[derive(Debug)]
pub struct CsrPem(pub String);

#[derive(Debug)]
pub struct CertChainPem {
    pub leaf: String,
    pub chain: Vec<String>,
}

#[derive(Debug)]
pub struct CrlPem(pub String);

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Principal {
    Node { node: String },
    Agent { node: String, agent: String },
    Device { user: String, device: String },
}

pub struct EnrollmentToken(pub String);

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "B14 requires redacted token Debug; deriving Debug would expose the enrollment secret"
    )
)]
impl std::fmt::Debug for EnrollmentToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("EnrollmentToken([redacted])")
    }
}

#[derive(Debug)]
pub enum SignAuthority {
    Token(EnrollmentToken),
    Renewal(Principal),
}

#[derive(Debug, thiserror::Error)]
pub enum CaError {
    #[error("certificate request refused: {0}")]
    Refused(String),
    #[error("invalid certificate request: {0}")]
    InvalidCsr(String),
    #[error("certificate authority unavailable: {0}")]
    Unavailable(#[source] BoxError),
}

#[async_trait]
pub trait CertificateAuthority: Send + Sync {
    async fn issue_token(&self, principal: Principal) -> Result<EnrollmentToken, CaError>;
    async fn sign(&self, csr: CsrPem, authority: SignAuthority) -> Result<CertChainPem, CaError>;
    async fn chain(&self) -> Result<Vec<String>, CaError>;
    async fn crl(&self) -> Result<CrlPem, CaError>;
    async fn revoke(&self, principal: Principal) -> Result<(), CaError>;
}
