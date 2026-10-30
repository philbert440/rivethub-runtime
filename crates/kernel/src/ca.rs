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

impl Principal {
    pub fn spiffe_uri(&self) -> Result<String, CaError> {
        let (kind, components) = match self {
            Self::Node { node } => ("node", vec![node]),
            Self::Agent { node, agent } => ("agent", vec![node, agent]),
            Self::Device { user, device } => ("device", vec![user, device]),
        };
        for component in &components {
            if component.is_empty()
                || component.len() > 128
                || component == &"."
                || component == &".."
                || !component.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~')
                })
            {
                return Err(CaError::Refused(
                    "principal components must be nonempty unreserved URI segments".to_owned(),
                ));
            }
        }
        Ok(format!(
            "spiffe://rivet.mesh/{kind}/{}",
            components
                .into_iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("/")
        ))
    }
}

impl std::str::FromStr for Principal {
    type Err = CaError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let path = value
            .strip_prefix("spiffe://rivet.mesh/")
            .ok_or_else(|| CaError::Refused("expected a RivetHub SPIFFE principal".to_owned()))?;
        let parts = path.split('/').collect::<Vec<_>>();
        let principal = match parts.as_slice() {
            ["node", node] => Self::Node {
                node: (*node).to_owned(),
            },
            ["agent", node, agent] => Self::Agent {
                node: (*node).to_owned(),
                agent: (*agent).to_owned(),
            },
            ["device", user, device] => Self::Device {
                user: (*user).to_owned(),
                device: (*device).to_owned(),
            },
            _ => return Err(CaError::Refused("invalid SPIFFE principal path".to_owned())),
        };
        principal.spiffe_uri()?;
        Ok(principal)
    }
}
