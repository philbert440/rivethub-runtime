use crate::Error;
use rcgen::PublicKeyData;
use rustls_pki_types::{CertificateDer, UnixTime};
use time::OffsetDateTime;
use x509_parser::prelude::FromDer;

pub(super) fn certificate(
    certificates: &[CertificateDer<'_>],
    root: &str,
    key: &rcgen::KeyPair,
    principal: &str,
    now: OffsetDateTime,
) -> Result<i64, Error> {
    let (leaf, intermediates) = certificates
        .split_first()
        .ok_or(Error::PkiInput("missing holder certificate"))?;
    let root = CertificateDer::from(pki::certificate(root)?);
    let anchor = webpki::anchor_from_trusted_cert(&root)
        .map_err(|_| Error::PkiInput("invalid trust root"))?;
    let timestamp =
        u64::try_from(now.unix_timestamp()).map_err(|_| Error::PkiInput("invalid current time"))?;
    webpki::EndEntityCert::try_from(leaf)
        .map_err(|_| Error::PkiInput("invalid holder certificate"))?
        .verify_for_usage(
            webpki::ALL_VERIFICATION_ALGS,
            &[anchor],
            intermediates,
            UnixTime::since_unix_epoch(std::time::Duration::from_secs(timestamp)),
            webpki::KeyUsage::client_auth(),
            None,
            None,
        )
        .map_err(|_| Error::PkiInput("holder certificate chain verification failed"))?;
    let (rest, certificate) = x509_parser::certificate::X509Certificate::from_der(leaf)
        .map_err(|_| Error::PkiInput("invalid holder certificate DER"))?;
    if !rest.is_empty() || certificate.public_key().raw != key.subject_public_key_info() {
        return Err(Error::PkiInput(
            "certificate does not match the local holder key",
        ));
    }
    let san = certificate
        .subject_alternative_name()
        .map_err(|_| Error::PkiInput("invalid holder SAN"))?
        .ok_or(Error::PkiInput("missing holder SAN"))?;
    if !matches!(san.value.general_names.as_slice(), [x509_parser::extensions::GeneralName::URI(uri)] if *uri == principal)
    {
        return Err(Error::PkiInput(
            "certificate must have exactly the holder SPIFFE URI SAN",
        ));
    }
    let start = certificate.validity().not_before.timestamp();
    let end = certificate.validity().not_after.timestamp();
    renewal_at(start, end)
}

pub(super) fn renewal_at(start: i64, end: i64) -> Result<i64, Error> {
    let lifetime = end
        .checked_sub(start)
        .filter(|value| *value > 0)
        .ok_or(Error::PkiInput("invalid certificate lifetime"))?;
    start
        .checked_add(lifetime / 3 * 2 + lifetime % 3 * 2 / 3)
        .ok_or(Error::PkiInput("invalid renewal deadline"))
}
