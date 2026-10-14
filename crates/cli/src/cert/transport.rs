use crate::Error;
use reqwest::{Certificate, Client, Identity};
use std::time::Duration;
use url::Url;
use wire::ca::{SignRequest, SignResponse};

pub(super) fn endpoint(mut base: Url) -> Result<Url, Error> {
    if base.scheme() != "https"
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
        || base.path() != "/"
    {
        return Err(Error::PkiInput(
            "CA URL must be an HTTPS origin without credentials, query or fragment",
        ));
    }
    base.set_path("/ca/v1/sign");
    Ok(base)
}

pub(super) async fn sign(
    endpoint: &Url,
    state: &super::state::Holder,
) -> Result<SignResponse, Error> {
    let origin = endpoint.origin();
    let mut client = Client::builder()
        .https_only(true)
        .no_proxy()
        .tls_certs_only([Certificate::from_pem(state.root.as_bytes()).map_err(Error::CaHttp)?])
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                return attempt.error("too many CA redirects");
            }
            if attempt.url().origin() == origin {
                attempt.follow()
            } else {
                attempt.error("CA redirect changed origin")
            }
        }));
    if let Some(certificate) = &state.certificate {
        let pem = format!("{certificate}\n{}", state.key.serialize_pem());
        client = client.identity(Identity::from_pem(pem.as_bytes()).map_err(Error::CaHttp)?);
    }
    let mut request = client
        .build()
        .map_err(Error::CaHttp)?
        .post(endpoint.clone())
        .json(&SignRequest {
            csr: state.csr.clone(),
        });
    if let Some(token) = &state.token {
        request = request.bearer_auth(&token.0);
    }
    let mut response = request.send().await.map_err(Error::CaHttp)?;
    if !response.status().is_success() {
        return Err(Error::CaStatus(response.status()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Error::CaHttp)? {
        if bytes.len() + chunk.len() > 1024 * 1024 {
            return Err(Error::PkiInput("CA response exceeds 1 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::PkiInput("invalid CA signing response"))
}
