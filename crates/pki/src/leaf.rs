use std::str::FromStr;

use kernel::ca::Principal;
use num_bigint::BigUint;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use x509_parser::certificate::X509Certificate;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::FromDer;

use crate::Error;

pub(crate) struct LeafIdentity {
    pub(crate) serial: String,
    pub(crate) principals: Vec<String>,
}

pub(crate) fn identity(pem: &str) -> Result<LeafIdentity, Error> {
    let der = CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|_| Error::Certificate)?;
    let (_, certificate) =
        X509Certificate::from_der(der.as_ref()).map_err(|_| Error::Certificate)?;
    let serial = decimal(certificate.raw_serial())?;
    let mut principals = Vec::new();
    if let Some(san) = certificate
        .subject_alternative_name()
        .map_err(|_| Error::Certificate)?
    {
        for name in &san.value.general_names {
            if let GeneralName::URI(uri) = name
                && uri.starts_with("spiffe://rivet.mesh/")
            {
                let principal = Principal::from_str(uri).map_err(|_| Error::Certificate)?;
                principals.push(principal.spiffe_uri().map_err(|_| Error::Certificate)?);
            }
        }
    }
    Ok(LeafIdentity { serial, principals })
}

fn decimal(raw: &[u8]) -> Result<String, Error> {
    if raw.is_empty() {
        return Err(Error::Serial);
    }
    let text = BigUint::from_bytes_be(raw).to_str_radix(10);
    let mut bytes = text.bytes();
    match bytes.next() {
        Some(first)
            if (b'1'..=b'9').contains(&first) && bytes.all(|byte| byte.is_ascii_digit()) =>
        {
            Ok(text)
        }
        _ => Err(Error::Serial),
    }
}
