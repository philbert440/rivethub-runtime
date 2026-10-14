use std::time::Duration;

use serde::Deserialize;
use tokio_util::sync::CancellationToken;
use url::Url;
use wire::gateway::GatewayError;
use wire::gateway::HealthzResponse;
use wire::gateway::sessions::SessionPostReply;
use wire::gateway::sessions::SessionPostRequest;

use crate::error::Error;
use crate::redirect::redirect_action;

#[derive(Clone)]
pub struct GatewayClient {
    http: reqwest::Client,
    base: Url,
    token: Option<String>,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "Debug names the base url and whether a token is set; it must not print the token"
    )
)]
impl std::fmt::Debug for GatewayClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GatewayClient")
            .field("base", &self.base)
            .field("has_token", &self.token.is_some())
            .finish_non_exhaustive()
    }
}

impl GatewayClient {
    pub fn new(base: Url, token: Option<String>) -> Result<Self, Error> {
        if base.host_str().is_none() {
            return Err(Error::BaseUrl);
        }
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::custom(redirect_action))
            .build()
            .map_err(|source| Error::Http(Box::new(source)))?;
        Ok(Self { http, base, token })
    }

    pub async fn healthz(&self) -> Result<HealthzResponse, Error> {
        self.healthz_at(&CancellationToken::new()).await
    }

    pub(crate) async fn healthz_at(
        &self,
        cancel: &CancellationToken,
    ) -> Result<HealthzResponse, Error> {
        self.get_typed("healthz", cancel).await
    }

    pub async fn sessions(&self) -> Result<wire::events::SessionList, Error> {
        let bytes = self
            .get_bytes("sessions", &CancellationToken::new())
            .await?;
        decode_sessions(&bytes)
    }

    pub async fn post_message(
        &self,
        session: &str,
        body: &SessionPostRequest,
        timeout: Option<Duration>,
        cancel: &CancellationToken,
    ) -> Result<SessionPostReply, Error> {
        check_segment(session)?;
        let path = format!("api/sessions/{session}/messages?wait=1");
        let response = self
            .send(
                self.http.post(self.endpoint(&path)?).json(body),
                timeout,
                cancel,
            )
            .await?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|source| Error::Http(Box::new(source)))?;
        if status.as_u16() == 200 {
            return serde_json::from_slice(&bytes).map_err(|_| Error::Response);
        }
        if status.as_u16() == 504 {
            return Err(Error::Timeout);
        }
        reject(status.as_u16(), &bytes)
    }

    pub(crate) async fn get_bytes(
        &self,
        path: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>, Error> {
        let response = self
            .send(self.http.get(self.endpoint(path)?), None, cancel)
            .await?;
        body_bytes(response).await
    }

    async fn get_typed<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        cancel: &CancellationToken,
    ) -> Result<T, Error> {
        let bytes = self.get_bytes(path, cancel).await?;
        serde_json::from_slice(&bytes).map_err(|_| Error::Response)
    }

    fn endpoint(&self, path: &str) -> Result<Url, Error> {
        if path.is_empty() || path.starts_with("//") || path.contains("://") {
            return Err(Error::SessionSegment);
        }
        self.base.join(path).map_err(|_| Error::BaseUrl)
    }

    async fn send(
        &self,
        mut request: reqwest::RequestBuilder,
        timeout: Option<Duration>,
        cancel: &CancellationToken,
    ) -> Result<reqwest::Response, Error> {
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(limit) = timeout {
            request = request.timeout(limit);
        }
        let built = request
            .build()
            .map_err(|source| Error::Http(Box::new(source)))?;
        tokio::select! {
            () = cancel.cancelled() => Err(Error::Cancelled),
            result = self.http.execute(built) => result.map_err(classify_transport),
        }
    }
}

pub(crate) fn check_segment(segment: &str) -> Result<(), Error> {
    if segment.is_empty()
        || segment == "."
        || segment == ".."
        || segment.contains('/')
        || segment.contains('\\')
    {
        return Err(Error::SessionSegment);
    }
    Ok(())
}

fn classify_transport(source: reqwest::Error) -> Error {
    if source.is_timeout() {
        Error::Timeout
    } else if source.is_redirect() {
        Error::Redirect(Box::new(source))
    } else {
        Error::Http(Box::new(source))
    }
}

async fn body_bytes(response: reqwest::Response) -> Result<Vec<u8>, Error> {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|source| Error::Http(Box::new(source)))?;
    if status.is_success() {
        return Ok(bytes.to_vec());
    }
    reject(status.as_u16(), &bytes)
}

fn reject<T>(status: u16, bytes: &[u8]) -> Result<T, Error> {
    if bytes.is_empty() {
        return Err(Error::Rejected {
            status,
            error: None,
        });
    }
    match decode_gateway_error(bytes) {
        Ok(error) => Err(Error::Rejected {
            status,
            error: Some(Box::new(error)),
        }),
        Err(Error::Response) => Err(Error::Response),
        Err(error) => Err(error),
    }
}

pub(crate) fn decode_sessions(bytes: &[u8]) -> Result<wire::events::SessionList, Error> {
    serde_json::from_slice(bytes).map_err(|_| Error::Response)
}

pub(crate) fn decode_gateway_error(bytes: &[u8]) -> Result<GatewayError, Error> {
    serde_json::from_slice(bytes).map_err(|_| Error::Response)
}
