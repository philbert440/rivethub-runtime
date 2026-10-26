use serde::Deserialize;

use rustls_pki_types::CertificateDer;
use rustls_pki_types::CertificateRevocationListDer;
use rustls_pki_types::CertificateSigningRequestDer;
use rustls_pki_types::UnixTime;
use rustls_pki_types::pem::PemObject;
use x509_parser::certificate::X509Certificate;
use x509_parser::certification_request::X509CertificationRequest;
use x509_parser::prelude::FromDer;
use x509_parser::revocation_list::CertificateRevocationList;

use crate::document::SignResponse;
use crate::document::SignedCertificate;
use crate::error::Error;
use crate::error::InvalidResponse;

pub(crate) fn one_root(pem: &str) -> Result<CertificateDer<'static>, Error> {
    let mut certificates = CertificateDer::pem_slice_iter(pem.as_bytes());
    let Some(first) = certificates.next() else {
        return Err(Error::Root);
    };
    let first = first.map_err(|_| Error::Root)?;
    if certificates.next().is_some() {
        return Err(Error::Root);
    }
    Ok(first)
}

pub(crate) fn csr(pem: &str) -> Result<(), Error> {
    let der =
        CertificateSigningRequestDer::from_pem_slice(pem.as_bytes()).map_err(|_| Error::Csr)?;
    let (rest, _) = X509CertificationRequest::from_der(der.as_ref()).map_err(|_| Error::Csr)?;
    if rest.is_empty() {
        Ok(())
    } else {
        Err(Error::Csr)
    }
}

pub(crate) fn token(value: &str) -> Result<&str, Error> {
    if value.is_empty() || value.bytes().any(|byte| byte.is_ascii_whitespace()) {
        Err(Error::Token)
    } else {
        Ok(value)
    }
}

pub(crate) fn serial(value: &str) -> Result<String, Error> {
    let digits = value.bytes().all(|byte| byte.is_ascii_digit());
    let canonical = digits && !value.is_empty() && (value.len() == 1 || !value.starts_with('0'));
    if canonical && value.len() <= 128 {
        Ok(value.to_owned())
    } else {
        Err(Error::Serial)
    }
}

pub(crate) fn signed(
    root: &CertificateDer<'_>,
    now: UnixTime,
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<SignedCertificate, Error> {
    require_json(content_type)?;
    let response: SignResponse = serde_json::from_slice(bytes).map_err(invalid_json)?;
    let leaf = certificate(&response.crt)?;
    let intermediate = certificate(&response.ca)?;
    link(&response, &leaf, &intermediate)?;
    trust(root, now, &leaf, &intermediate)?;
    Ok(SignedCertificate {
        leaf: response.crt,
        intermediate: response.ca,
        chain: response.cert_chain,
        tls: response.tls_options,
    })
}

pub(crate) fn roots(
    root: &CertificateDer<'_>,
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<Vec<String>, Error> {
    require_json(content_type)?;
    let parsed: Roots = serde_json::from_slice(bytes).map_err(invalid_json)?;
    if parsed.crts.is_empty() {
        return Err(Error::Invalid(InvalidResponse::Roots));
    }
    for pem in &parsed.crts {
        let der = certificate(pem).map_err(|_| Error::Invalid(InvalidResponse::Roots))?;
        if der.as_ref() != root.as_ref() {
            return Err(Error::Invalid(InvalidResponse::Roots));
        }
    }
    Ok(parsed.crts)
}

pub(crate) fn ack(content_type: Option<&str>, bytes: &[u8]) -> Result<(), Error> {
    require_json(content_type)?;
    let parsed: AckBody = serde_json::from_slice(bytes).map_err(invalid_json)?;
    match parsed.status {
        Ack::Ok => Ok(()),
    }
}

pub(crate) fn crl(bytes: &[u8]) -> Result<String, Error> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Invalid(InvalidResponse::Encoding))?;
    let der = CertificateRevocationListDer::from_pem_slice(bytes)
        .map_err(|_| Error::Invalid(InvalidResponse::Crl))?;
    let (rest, _) = CertificateRevocationList::from_der(der.as_ref())
        .map_err(|_| Error::Invalid(InvalidResponse::Crl))?;
    if rest.is_empty() {
        Ok(text.to_owned())
    } else {
        Err(Error::Invalid(InvalidResponse::Crl))
    }
}

pub(crate) fn failure(status: u16, content_type: Option<&str>, bytes: &[u8]) -> Error {
    if (200..300).contains(&status) {
        return Error::Invalid(InvalidResponse::UnexpectedStatus(status));
    }
    if let Err(error) = require_json(content_type) {
        return error;
    }
    let document: Failure = match serde_json::from_slice(bytes) {
        Ok(document) => document,
        Err(source) => return Error::Invalid(InvalidResponse::Json(source)),
    };
    if document.status != status {
        return Error::Invalid(InvalidResponse::StatusMismatch {
            http: status,
            body: document.status,
        });
    }
    if document.message.is_empty() {
        return Error::Invalid(InvalidResponse::EmptyMessage);
    }
    match status {
        400 => Error::Rejected {
            status,
            message: document.message,
        },
        401 | 403 => Error::Refused {
            status,
            message: document.message,
        },
        _ => Error::Unavailable {
            status,
            message: document.message,
        },
    }
}

fn link(
    response: &SignResponse,
    leaf: &CertificateDer<'_>,
    intermediate: &CertificateDer<'_>,
) -> Result<(), Error> {
    let Some(chain_leaf) = response.cert_chain.first() else {
        return Err(Error::Invalid(InvalidResponse::Chain));
    };
    let Some(chain_issuer) = response.cert_chain.get(1) else {
        return Err(Error::Invalid(InvalidResponse::Chain));
    };
    if response.cert_chain.len() != 2 {
        return Err(Error::Invalid(InvalidResponse::Chain));
    }
    if certificate(chain_leaf)?.as_ref() != leaf.as_ref()
        || certificate(chain_issuer)?.as_ref() != intermediate.as_ref()
    {
        return Err(Error::Invalid(InvalidResponse::Chain));
    }
    Ok(())
}

fn trust(
    root: &CertificateDer<'_>,
    now: UnixTime,
    leaf: &CertificateDer<'_>,
    intermediate: &CertificateDer<'_>,
) -> Result<(), Error> {
    let anchor = webpki::anchor_from_trusted_cert(root)
        .map_err(|error| Error::Untrusted(error.to_string()))?;
    let end = webpki::EndEntityCert::try_from(leaf)
        .map_err(|error| Error::Untrusted(error.to_string()))?;
    let chain = [intermediate.clone()];
    end.verify_for_usage(
        webpki::ALL_VERIFICATION_ALGS,
        &[anchor],
        &chain,
        now,
        webpki::KeyUsage::client_auth(),
        None,
        None,
    )
    .map(|_| ())
    .map_err(|error| Error::Untrusted(error.to_string()))
}

fn certificate(pem: &str) -> Result<CertificateDer<'static>, Error> {
    let der = CertificateDer::from_pem_slice(pem.as_bytes())
        .map_err(|_| Error::Invalid(InvalidResponse::Chain))?;
    let (rest, _) = X509Certificate::from_der(der.as_ref())
        .map_err(|_| Error::Invalid(InvalidResponse::Chain))?;
    if rest.is_empty() {
        Ok(der)
    } else {
        Err(Error::Invalid(InvalidResponse::Chain))
    }
}

fn require_json(content_type: Option<&str>) -> Result<(), Error> {
    let media = content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    if media.is_some_and(|value| value.eq_ignore_ascii_case("application/json")) {
        Ok(())
    } else {
        Err(Error::Invalid(InvalidResponse::NotJson))
    }
}

fn invalid_json(source: serde_json::Error) -> Error {
    Error::Invalid(InvalidResponse::Json(source))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Roots {
    crts: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Failure {
    status: u16,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AckBody {
    status: Ack,
}

#[derive(Debug, Deserialize)]
enum Ack {
    #[serde(rename = "ok")]
    Ok,
}
