use crate::error::AddressFault;
use crate::error::Error;
use url::Url;

pub(crate) fn origin(mut address: Url) -> Result<Url, Error> {
    if address.scheme() != "https" {
        return Err(Error::Address(AddressFault::Scheme));
    }
    if address.host_str().is_none() {
        return Err(Error::Address(AddressFault::Host));
    }
    if !address.username().is_empty() || address.password().is_some() {
        return Err(Error::Address(AddressFault::Credentials));
    }
    if address.query().is_some() {
        return Err(Error::Address(AddressFault::Query));
    }
    if address.fragment().is_some() {
        return Err(Error::Address(AddressFault::Fragment));
    }
    if address.path() != "/" && !address.path().is_empty() {
        return Err(Error::Address(AddressFault::Path));
    }
    address.set_path("/");
    Ok(address)
}

pub(crate) fn join(address: &Url, path: &str, query: Option<&str>) -> Url {
    let mut url = address.clone();
    url.set_path(path);
    url.set_query(query);
    url
}

pub(crate) fn redirect(
    origin: &url::Origin,
    attempt: reqwest::redirect::Attempt<'_>,
) -> reqwest::redirect::Action {
    if attempt.previous().len() >= 5 {
        return attempt.error("too many step-ca redirects");
    }
    if &attempt.url().origin() != origin {
        return attempt.error("step-ca redirect left the configured origin");
    }
    if matches!(attempt.status().as_u16(), 307 | 308) {
        attempt.follow()
    } else {
        attempt.error("step-ca redirect does not preserve the request")
    }
}
