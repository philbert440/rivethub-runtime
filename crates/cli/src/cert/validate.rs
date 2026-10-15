use crate::Error;
use rcgen::PublicKeyData;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, UnixTime};
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use time::OffsetDateTime;
use x509_parser::extensions::GeneralName;
use x509_parser::prelude::FromDer;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Name {
    Dns(String),
    Ip(IpAddr),
    Uri(String),
}

pub(super) struct Accepted {
    pub(super) names: BTreeSet<Name>,
    pub(super) renew_at: i64,
}

pub(super) fn name(value: &str) -> Result<Name, Error> {
    if value.is_empty() || value.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(Error::PkiInput(
            "subject alternative name is empty or contains whitespace",
        ));
    }
    if value.contains("://") {
        return Ok(Name::Uri(value.to_owned()));
    }
    if value
        .bytes()
        .all(|byte| byte.is_ascii_digit() || byte == b'.')
        && value.contains('.')
    {
        let ip = value
            .parse::<IpAddr>()
            .map_err(|_| Error::PkiInput("subject alternative name is not a valid IP address"))?;
        return Ok(Name::Ip(ip));
    }
    if value.contains(':') {
        let ip = value
            .parse::<IpAddr>()
            .map_err(|_| Error::PkiInput("subject alternative name is not a valid IP address"))?;
        return Ok(Name::Ip(ip));
    }
    Ok(Name::Dns(value.to_owned()))
}

pub(super) fn principal(uri: &str) -> Result<Name, Error> {
    match name(uri)? {
        Name::Uri(value) => Ok(Name::Uri(value)),
        Name::Dns(_) | Name::Ip(_) => Err(Error::PkiInput("holder principal must be a SPIFFE URI")),
    }
}

pub(super) fn foreign_uri(names: &[Name], principal: &Name) -> Result<(), Error> {
    if names
        .iter()
        .any(|name| matches!(name, Name::Uri(_)) && name != principal)
    {
        Err(Error::PkiInput(
            "enrollment token pins a different URI than the holder principal",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn establish(
    pem: &str,
    root: &str,
    key: &rcgen::KeyPair,
    principal: &Name,
    now: OffsetDateTime,
) -> Result<Accepted, Error> {
    let accepted = checked(pem, root, key, now)?;
    one_identity(&accepted.names, principal)?;
    Ok(accepted)
}

pub(super) fn accept(
    pem: &str,
    root: &str,
    key: &rcgen::KeyPair,
    requested: &BTreeSet<Name>,
    principal: &Name,
    now: OffsetDateTime,
) -> Result<Accepted, Error> {
    let accepted = checked(pem, root, key, now)?;
    one_identity(&accepted.names, principal)?;
    if !requested.is_subset(&accepted.names) {
        return Err(Error::PkiInput(
            "certificate is missing a subject alternative name pinned by the token",
        ));
    }
    if accepted
        .names
        .iter()
        .any(|name| name != principal && !requested.contains(name))
    {
        return Err(Error::PkiInput(
            "certificate contains a subject alternative name that was not requested",
        ));
    }
    Ok(accepted)
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

fn checked(
    pem: &str,
    root: &str,
    key: &rcgen::KeyPair,
    now: OffsetDateTime,
) -> Result<Accepted, Error> {
    if pem.contains("PRIVATE KEY") {
        return Err(Error::PkiInput("certificate chain contains a private key"));
    }
    let ders = CertificateDer::pem_slice_iter(pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| Error::PkiInput("invalid holder certificate PEM"))?;
    let (leaf, intermediates) = ders
        .split_first()
        .ok_or(Error::PkiInput("missing holder certificate"))?;
    trust(leaf, intermediates, root, now)?;
    let (rest, certificate) = x509_parser::certificate::X509Certificate::from_der(leaf)
        .map_err(|_| Error::PkiInput("invalid holder certificate DER"))?;
    if !rest.is_empty() || certificate.public_key().raw != key.subject_public_key_info() {
        return Err(Error::PkiInput(
            "certificate does not match the local holder key",
        ));
    }
    let names = subject_names(&certificate)?;
    let start = certificate.validity().not_before.timestamp();
    let end = certificate.validity().not_after.timestamp();
    let renew_at = renewal_at(start, end)?;
    if renew_at <= now.unix_timestamp() {
        return Err(Error::PkiInput(
            "CA certificate has already reached its renewal deadline",
        ));
    }
    Ok(Accepted { names, renew_at })
}

fn trust(
    leaf: &CertificateDer<'_>,
    intermediates: &[CertificateDer<'_>],
    root: &str,
    now: OffsetDateTime,
) -> Result<(), Error> {
    let root = CertificateDer::from_pem_slice(root.as_bytes())
        .map_err(|_| Error::PkiInput("invalid trust root"))?;
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
    Ok(())
}

fn subject_names(
    certificate: &x509_parser::certificate::X509Certificate<'_>,
) -> Result<BTreeSet<Name>, Error> {
    let san = certificate
        .subject_alternative_name()
        .map_err(|_| Error::PkiInput("invalid holder SAN"))?
        .ok_or(Error::PkiInput("missing holder SAN"))?;
    let mut names = BTreeSet::new();
    for general in &san.value.general_names {
        let name = general_name(general)?;
        if !names.insert(name) {
            return Err(Error::PkiInput(
                "certificate repeats a subject alternative name",
            ));
        }
    }
    if names.is_empty() {
        Err(Error::PkiInput("missing holder SAN"))
    } else {
        Ok(names)
    }
}

fn general_name(general: &GeneralName<'_>) -> Result<Name, Error> {
    match general {
        GeneralName::DNSName(value) | GeneralName::URI(value) => name(value),
        GeneralName::IPAddress(bytes) => Ok(Name::Ip(address(bytes)?)),
        GeneralName::OtherName(_, _)
        | GeneralName::RFC822Name(_)
        | GeneralName::X400Address(_)
        | GeneralName::DirectoryName(_)
        | GeneralName::EDIPartyName(_)
        | GeneralName::RegisteredID(_)
        | GeneralName::Invalid(_, _) => Err(Error::PkiInput(
            "certificate subject alternative name is not a DNS name, IP address or URI",
        )),
    }
}

fn address(bytes: &[u8]) -> Result<IpAddr, Error> {
    match bytes.len() {
        4 => {
            let mut octets = [0; 4];
            octets.copy_from_slice(bytes);
            Ok(IpAddr::V4(Ipv4Addr::from(octets)))
        }
        16 => {
            let mut octets = [0; 16];
            octets.copy_from_slice(bytes);
            Ok(IpAddr::V6(Ipv6Addr::from(octets)))
        }
        _ => Err(Error::PkiInput(
            "certificate IP address has an invalid length",
        )),
    }
}

fn one_identity(names: &BTreeSet<Name>, principal: &Name) -> Result<(), Error> {
    let mut uris = names.iter().filter(|name| matches!(name, Name::Uri(_)));
    let identity = uris.next();
    if identity == Some(principal) && uris.next().is_none() {
        Ok(())
    } else {
        Err(Error::PkiInput(
            "certificate must contain exactly the holder SPIFFE URI",
        ))
    }
}
