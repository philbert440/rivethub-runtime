use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressFault {
    Scheme,
    Host,
    Credentials,
    Query,
    Fragment,
    Path,
}

impl fmt::Display for AddressFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = match self {
            Self::Scheme => "scheme must be https",
            Self::Host => "host is missing",
            Self::Credentials => "credentials are not allowed",
            Self::Query => "query is not allowed",
            Self::Fragment => "fragment is not allowed",
            Self::Path => "path must be empty",
        };
        formatter.write_str(detail)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidResponse {
    #[error("step-ca response exceeds 1 MiB")]
    TooLarge,
    #[error("step-ca response is not JSON")]
    NotJson,
    #[error("step-ca response JSON is not the expected document")]
    Json(#[source] serde_json::Error),
    #[error("step-ca HTTP status {http} does not match the document status {body}")]
    StatusMismatch { http: u16, body: u16 },
    #[error("step-ca returned HTTP {0} for an exchange that requires a different success status")]
    UnexpectedStatus(u16),
    #[error("step-ca error message is empty")]
    EmptyMessage,
    #[error("step-ca certificate chain does not link the issued certificate")]
    Chain,
    #[error("step-ca response is not UTF-8")]
    Encoding,
    #[error("step-ca revocation list is not a PEM X.509 CRL")]
    Crl,
    #[error("step-ca roots do not match the configured root")]
    Roots,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("step-ca address is not an https origin ({0})")]
    Address(AddressFault),
    #[error("step-ca trust root is not one PEM certificate")]
    Root,
    #[error("certificate signing request is not a PEM PKCS#10 request")]
    Csr,
    #[error("one-time token is empty or contains whitespace")]
    Token,
    #[error("revocation serial is not a canonical decimal integer")]
    Serial,
    #[error("holder certificate is not a PEM certificate")]
    HolderCertificate,
    #[error("holder identity is not a PEM certificate and private key")]
    Holder(#[source] reqwest::Error),
    #[error("step-ca refused the request ({status}): {message}")]
    Refused { status: u16, message: String },
    #[error("step-ca rejected the request ({status}): {message}")]
    Rejected { status: u16, message: String },
    #[error("step-ca is unavailable ({status}): {message}")]
    Unavailable { status: u16, message: String },
    #[error("step-ca request was cancelled")]
    Cancelled,
    #[error("system clock is not usable for certificate validity")]
    Clock,
    #[error("step-ca transport failed")]
    Transport(#[source] reqwest::Error),
    #[error(transparent)]
    Invalid(InvalidResponse),
    #[error("certificate is not trusted by the configured step-ca root: {0}")]
    Untrusted(String),
}
