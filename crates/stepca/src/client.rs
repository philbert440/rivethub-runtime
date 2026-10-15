use std::time::Duration;

use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::document::Revocation;
use crate::document::RevokeBody;
use crate::document::SignBody;
use crate::document::SignedCertificate;
use crate::endpoint;
use crate::error::Error;
use crate::verify;

const BODY_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Config {
    address: Url,
    root_pem: String,
}

impl Config {
    pub fn new(address: Url, root_pem: String) -> Result<Self, Error> {
        let address = endpoint::origin(address)?;
        verify::one_root(&root_pem)?;
        drop(reqwest::Certificate::from_pem(root_pem.as_bytes()).map_err(|_| Error::Root)?);
        Ok(Self { address, root_pem })
    }
}

#[derive(Debug, Clone)]
pub struct Holder {
    identity: reqwest::Identity,
}

impl Holder {
    pub fn from_pem(certificate_pem: &str, private_key_pem: &str) -> Result<Self, Error> {
        drop(
            CertificateDer::from_pem_slice(certificate_pem.as_bytes())
                .map_err(|_| Error::HolderCertificate)?,
        );
        let identity =
            reqwest::Identity::from_pem(format!("{certificate_pem}\n{private_key_pem}").as_bytes())
                .map_err(Error::Holder)?;
        Ok(Self { identity })
    }
}

#[derive(Debug, Clone)]
pub struct Authority {
    address: Url,
    origin: url::Origin,
    root: CertificateDer<'static>,
    trust: reqwest::Certificate,
    anonymous: reqwest::Client,
    cancel: CancellationToken,
}

impl Authority {
    pub fn connect(config: Config) -> Result<Self, Error> {
        Self::assemble(config.address, &config.root_pem, &config.root_pem)
    }

    #[must_use]
    pub fn with_cancellation(mut self, cancel: CancellationToken) -> Self {
        self.cancel = cancel;
        self
    }

    pub async fn sign(&self, csr_pem: &str, token: &str) -> Result<SignedCertificate, Error> {
        verify::csr(csr_pem)?;
        let token = verify::token(token)?;
        let builder = self
            .builder(None, reqwest::Method::POST, "/1.0/sign", None)?
            .json(&SignBody {
                csr: csr_pem,
                ott: token,
            });
        let received = self.exchange(builder, 201).await?;
        verify::signed(&self.root, received.content_type.as_deref(), &received.body)
    }

    pub async fn renew(&self, holder: &Holder) -> Result<SignedCertificate, Error> {
        let builder = self.builder(Some(holder), reqwest::Method::POST, "/1.0/renew", None)?;
        let received = self.exchange(builder, 201).await?;
        verify::signed(&self.root, received.content_type.as_deref(), &received.body)
    }

    pub async fn revoke(&self, holder: &Holder, revocation: &Revocation) -> Result<(), Error> {
        let builder = self
            .builder(Some(holder), reqwest::Method::POST, "/1.0/revoke", None)?
            .json(&RevokeBody {
                serial: revocation.serial.as_str(),
                reason_code: revocation.reason.code(),
                reason: revocation.reason,
                passive: revocation.passive,
            });
        let received = self.exchange(builder, 200).await?;
        verify::ack(received.content_type.as_deref(), &received.body)
    }

    pub async fn roots(&self) -> Result<Vec<String>, Error> {
        let builder = self.builder(None, reqwest::Method::GET, "/roots", None)?;
        let received = self.exchange(builder, 201).await?;
        verify::roots(&self.root, received.content_type.as_deref(), &received.body)
    }

    pub async fn crl(&self) -> Result<String, Error> {
        let builder = self.builder(None, reqwest::Method::GET, "/1.0/crl", Some("pem=true"))?;
        let received = self.exchange(builder, 200).await?;
        verify::crl(&received.body)
    }

    #[cfg(test)]
    pub(crate) fn for_replay(address: Url, ca_pem: &str, tls_pem: &str) -> Result<Self, Error> {
        let address = endpoint::origin(address)?;
        Self::assemble(address, ca_pem, tls_pem)
    }

    #[cfg(test)]
    pub(crate) async fn renew_without_holder(&self) -> Result<SignedCertificate, Error> {
        let builder = self.builder(None, reqwest::Method::POST, "/1.0/renew", None)?;
        let received = self.exchange(builder, 201).await?;
        verify::signed(&self.root, received.content_type.as_deref(), &received.body)
    }

    fn assemble(address: Url, ca_pem: &str, tls_pem: &str) -> Result<Self, Error> {
        let root = verify::one_root(ca_pem)?;
        let trust = reqwest::Certificate::from_pem(tls_pem.as_bytes()).map_err(|_| Error::Root)?;
        let origin = address.origin();
        let anonymous = http_client(&trust, &origin, None)?;
        Ok(Self {
            address,
            origin,
            root,
            trust,
            anonymous,
            cancel: CancellationToken::new(),
        })
    }

    fn builder(
        &self,
        holder: Option<&Holder>,
        method: reqwest::Method,
        path: &str,
        query: Option<&str>,
    ) -> Result<reqwest::RequestBuilder, Error> {
        let client = match holder {
            Some(holder) => http_client(&self.trust, &self.origin, Some(holder.identity.clone()))?,
            None => self.anonymous.clone(),
        };
        Ok(client.request(method, endpoint::join(&self.address, path, query)))
    }

    async fn exchange(
        &self,
        builder: reqwest::RequestBuilder,
        expected: u16,
    ) -> Result<Received, Error> {
        let operation = async move {
            let response = builder.send().await.map_err(Error::Transport)?;
            let status = response.status().as_u16();
            let content_type = content_type(&response)?;
            let body = read_limited(response).await?;
            if status == expected {
                Ok(Received { content_type, body })
            } else {
                Err(verify::failure(status, content_type.as_deref(), &body))
            }
        };
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Err(Error::Cancelled),
            result = operation => result,
        }
    }
}

struct Received {
    content_type: Option<String>,
    body: Vec<u8>,
}

fn http_client(
    trust: &reqwest::Certificate,
    origin: &url::Origin,
    identity: Option<reqwest::Identity>,
) -> Result<reqwest::Client, Error> {
    let origin = origin.clone();
    let mut builder = reqwest::Client::builder()
        .https_only(true)
        .no_proxy()
        .tls_certs_only([trust.clone()])
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            endpoint::redirect(&origin, attempt)
        }));
    if let Some(identity) = identity {
        builder = builder.identity(identity);
    }
    builder.build().map_err(Error::Transport)
}

fn content_type(response: &reqwest::Response) -> Result<Option<String>, Error> {
    match response.headers().get(reqwest::header::CONTENT_TYPE) {
        None => Ok(None),
        Some(value) => {
            let text = value
                .to_str()
                .map_err(|_| Error::Invalid(crate::error::InvalidResponse::NotJson))?;
            Ok(Some(text.to_owned()))
        }
    }
}

async fn read_limited(mut response: reqwest::Response) -> Result<Vec<u8>, Error> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
        if body.len().saturating_add(chunk.len()) > BODY_LIMIT {
            return Err(Error::Invalid(crate::error::InvalidResponse::TooLarge));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
