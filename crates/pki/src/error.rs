use kernel::ca::CaError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("PKI storage: {0}")]
    Io(#[from] std::io::Error),
    #[error("PKI database: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("PKI PEM: {0}")]
    Pem(#[from] rustls_pki_types::pem::Error),
    #[error("PKI identity encoding: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("PKI cryptography: {0}")]
    Crypto(#[from] rcgen::Error),
    #[error("PKI time: {0}")]
    Time(#[from] time::error::ComponentRange),
    #[error("invalid CA import: {0}")]
    Import(String),
    #[error("PKI operation cancelled")]
    Cancelled,
    #[error("PKI task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
    #[error("PKI entropy: {0}")]
    Entropy(getrandom::Error),
    #[error(transparent)]
    Request(#[from] CaError),
}

impl From<Error> for CaError {
    fn from(error: Error) -> Self {
        match error {
            Error::Request(error) => error,
            other @ (Error::Io(_)
            | Error::Pem(_)
            | Error::Database(_)
            | Error::Encoding(_)
            | Error::Crypto(_)
            | Error::Time(_)
            | Error::Import(_)
            | Error::Cancelled
            | Error::Task(_)
            | Error::Entropy(_)) => Self::Unavailable(Box::new(other)),
        }
    }
}

pub(crate) fn refused(message: &str) -> Error {
    CaError::Refused(message.to_owned()).into()
}

pub(crate) fn invalid(message: &str) -> Error {
    CaError::InvalidCsr(message.to_string()).into()
}
