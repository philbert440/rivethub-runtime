pub(crate) mod common;
pub(crate) mod server;

pub(crate) use common::{Result, directory, run, text, token};

use rustls_pki_types::pem::PemObject;
use serde::Deserialize;
use x509_parser::prelude::FromDer;

pub(crate) fn csr_names(pem: &str) -> Result<Vec<String>> {
    let der = rustls_pki_types::CertificateSigningRequestDer::from_pem_slice(pem.as_bytes())?;
    let (_, request) =
        x509_parser::certification_request::X509CertificationRequest::from_der(&der)?;
    let Some(extensions) = request.requested_extensions() else {
        return Err("certificate request has no names".into());
    };
    let mut names = Vec::new();
    for extension in extensions {
        if let x509_parser::extensions::ParsedExtension::SubjectAlternativeName(san) = extension {
            for general in &san.general_names {
                names.push(general_name(general)?);
            }
        }
    }
    if names.is_empty() {
        Err("certificate request has no names".into())
    } else {
        Ok(names)
    }
}

pub(crate) fn recording() -> Result<Recording> {
    let path = testkit::recording("stepca/api.json");
    let catalog: Catalog = serde_json::from_slice(&std::fs::read(path)?)?;
    if catalog.crl_pem.is_empty() {
        return Err("recorded CRL is empty".into());
    }
    let mut cases = Vec::with_capacity(catalog.cases.len());
    for case in catalog.cases {
        cases.push(RecordedCase {
            id: case.id,
            method: case.request.method,
            path: case.request.path,
            client_certificate: case.request.client_certificate,
            status: case.response.status,
            message: case.response.json.message,
            tls: case.response.json.tls_options,
        });
    }
    Ok(Recording { cases })
}

pub(crate) struct Recording {
    pub(crate) cases: Vec<RecordedCase>,
}

pub(crate) struct RecordedCase {
    pub(crate) id: String,
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) client_certificate: String,
    pub(crate) status: u16,
    pub(crate) message: Option<String>,
    pub(crate) tls: Option<stepca::TlsOptions>,
}

impl Recording {
    pub(crate) fn case(&self, id: &str) -> Result<&RecordedCase> {
        self.cases
            .iter()
            .find(|case| case.id == id)
            .ok_or_else(|| format!("missing recorded case {id}").into())
    }
}

fn general_name(general: &x509_parser::extensions::GeneralName<'_>) -> Result<String> {
    match general {
        x509_parser::extensions::GeneralName::DNSName(value)
        | x509_parser::extensions::GeneralName::URI(value) => Ok((*value).to_owned()),
        x509_parser::extensions::GeneralName::IPAddress(bytes) => Ok(address(bytes)?),
        x509_parser::extensions::GeneralName::OtherName(_, _)
        | x509_parser::extensions::GeneralName::RFC822Name(_)
        | x509_parser::extensions::GeneralName::X400Address(_)
        | x509_parser::extensions::GeneralName::DirectoryName(_)
        | x509_parser::extensions::GeneralName::EDIPartyName(_)
        | x509_parser::extensions::GeneralName::RegisteredID(_)
        | x509_parser::extensions::GeneralName::Invalid(_, _) => {
            Err("unsupported certificate request name".into())
        }
    }
}

fn address(bytes: &[u8]) -> Result<String> {
    match bytes.len() {
        4 => {
            let mut octets = [0; 4];
            octets.copy_from_slice(bytes);
            Ok(std::net::IpAddr::V4(std::net::Ipv4Addr::from(octets)).to_string())
        }
        16 => {
            let mut octets = [0; 16];
            octets.copy_from_slice(bytes);
            Ok(std::net::IpAddr::V6(std::net::Ipv6Addr::from(octets)).to_string())
        }
        _ => Err("certificate request IP address has an invalid length".into()),
    }
}

#[derive(Deserialize)]
struct Catalog {
    cases: Vec<CatalogCase>,
    #[serde(rename = "crlPem")]
    crl_pem: String,
}

#[derive(Deserialize)]
struct CatalogCase {
    id: String,
    request: CatalogRequest,
    response: CatalogResponse,
}

#[derive(Deserialize)]
struct CatalogRequest {
    method: String,
    path: String,
    #[serde(rename = "clientCertificate")]
    client_certificate: String,
}

#[derive(Deserialize)]
struct CatalogResponse {
    status: u16,
    json: CatalogBody,
}

#[derive(Deserialize)]
struct CatalogBody {
    #[serde(default)]
    message: Option<String>,
    #[serde(default, rename = "tlsOptions")]
    tls_options: Option<stepca::TlsOptions>,
}
