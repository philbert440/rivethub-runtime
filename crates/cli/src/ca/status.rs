use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use tokio_util::sync::CancellationToken;
use x509_parser::certificate::X509Certificate;
use x509_parser::prelude::FromDer;

use super::paths::Material;
use super::step;
use crate::{Error, Report};

pub(super) async fn report(
    material: &Material,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    let roots = step::connect(material, shutdown)?.roots().await?;
    let text = tokio::task::spawn_blocking(move || describe(&roots)).await??;
    Ok(Report::success(text))
}

fn describe(chain: &[String]) -> Result<String, Error> {
    use std::fmt::Write;
    let mut output = String::from("Certificate authority ready.\n");
    for pem in chain {
        let der =
            CertificateDer::from_pem_slice(pem.as_bytes()).map_err(|_| Error::CaCertificate)?;
        let (rest, certificate) =
            X509Certificate::from_der(der.as_ref()).map_err(|_| Error::CaCertificate)?;
        if !rest.is_empty() {
            return Err(Error::CaCertificate);
        }
        let subject = certificate.subject();
        let expires = certificate.validity().not_after;
        writeln!(output, "{subject}: expires {expires}").map_err(|_| Error::CaCertificate)?;
    }
    Ok(output)
}
