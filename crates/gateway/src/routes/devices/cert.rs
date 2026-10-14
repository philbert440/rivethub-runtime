use std::path::Path;

use chrono::Datelike;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::DnType;
use rcgen::DnValue;
use rcgen::ExtendedKeyUsagePurpose;
use rcgen::IsCa;
use rcgen::KeyPair;
use rcgen::KeyUsagePurpose;
use rustls::pki_types::CertificateDer;
use rustls::pki_types::UnixTime;
use webpki::EndEntityCert;
use webpki::KeyUsage;
use x509_parser::certificate::X509Certificate;
use x509_parser::prelude::FromDer;

use super::CA_UNUSABLE;
use super::DeviceError;
use super::random;

pub(super) struct IssuedCert {
    pub p12_der: Vec<u8>,
    pub passphrase: String,
}

pub(super) fn issue_client(ca_dir: &Path, device_id: &str) -> Result<IssuedCert, DeviceError> {
    let authority = Authority::load(ca_dir)?;
    let key = KeyPair::generate().map_err(|error| {
        tracing::warn!(%error, "device key generation failed");
        DeviceError::Unavailable(CA_UNUSABLE)
    })?;
    let passphrase = random::passphrase()?;
    let leaf = sign_leaf(&authority, device_id, &key)?;
    verify_leaf(leaf.der().as_ref(), &authority)?;
    let p12_der = package_p12(
        leaf.der().as_ref(),
        &key.serialize_der(),
        &authority,
        &passphrase,
        device_id,
    )?;
    Ok(IssuedCert {
        p12_der,
        passphrase,
    })
}

pub(super) fn authority_usable(ca_dir: &Path) -> bool {
    Authority::load(ca_dir).is_ok()
}

struct Authority {
    key: KeyPair,
    issuer: rcgen::Certificate,
    intermediate_der: Vec<u8>,
    root_der: Vec<u8>,
}

impl Authority {
    fn load(ca_dir: &Path) -> Result<Self, DeviceError> {
        let intermediate_key = read_text(&ca_dir.join("intermediate").join("key.pem"))?;
        let key = KeyPair::from_pem(&intermediate_key).map_err(|error| {
            tracing::warn!(%error, "device intermediate key is unusable");
            DeviceError::Unavailable(CA_UNUSABLE)
        })?;
        let intermediate_der =
            one_cert(&read_bytes(&ca_dir.join("intermediate").join("cert.pem"))?)?;
        let root_der = one_cert(&read_bytes(&ca_dir.join("root").join("cert.pem"))?)?;
        let chain = pem_certs(&read_bytes(&ca_dir.join("intermediate").join("chain.pem"))?)?;
        if !chain.iter().any(|cert| cert == &intermediate_der) {
            tracing::warn!("device intermediate chain does not contain the intermediate");
            return Err(DeviceError::Unavailable(CA_UNUSABLE));
        }
        let issuer = issuer_carrier(&intermediate_der, &key)?;
        Ok(Self {
            key,
            issuer,
            intermediate_der,
            root_der,
        })
    }
}

fn issuer_carrier(
    intermediate_der: &[u8],
    key: &KeyPair,
) -> Result<rcgen::Certificate, DeviceError> {
    let mut params = CertificateParams::default();
    params.distinguished_name = distinguished_name(intermediate_der)?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let carrier = params.self_signed(key).map_err(|error| {
        tracing::warn!(%error, "device issuer parameters were rejected");
        DeviceError::Unavailable(CA_UNUSABLE)
    })?;
    if !subject_matches(carrier.der().as_ref(), intermediate_der) {
        tracing::warn!("device issuer name does not match the intermediate certificate");
        return Err(DeviceError::Unavailable(CA_UNUSABLE));
    }
    Ok(carrier)
}

fn sign_leaf(
    authority: &Authority,
    device_id: &str,
    key: &KeyPair,
) -> Result<rcgen::Certificate, DeviceError> {
    let mut params = CertificateParams::default();
    apply_validity(&mut params)?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(DnType::CommonName, device_id);
    params
        .distinguished_name
        .push(DnType::OrganizationalUnitName, "client");
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    params
        .signed_by(key, &authority.issuer, &authority.key)
        .map_err(|error| {
            tracing::warn!(%error, "device certificate signing failed");
            DeviceError::Unavailable(CA_UNUSABLE)
        })
}

fn apply_validity(params: &mut CertificateParams) -> Result<(), DeviceError> {
    let year = chrono::Utc::now().year();
    let before = year.checked_sub(1).ok_or(DeviceError::Internal)?;
    let after = year.checked_add(2).ok_or(DeviceError::Internal)?;
    params.not_before = rcgen::date_time_ymd(before, 1, 1);
    params.not_after = rcgen::date_time_ymd(after, 1, 1);
    Ok(())
}

fn verify_leaf(leaf: &[u8], authority: &Authority) -> Result<(), DeviceError> {
    let leaf_der = CertificateDer::from(leaf.to_vec());
    let end = EndEntityCert::try_from(&leaf_der).map_err(|error| {
        tracing::warn!(%error, "issued device certificate did not parse");
        DeviceError::Unavailable(CA_UNUSABLE)
    })?;
    let root_der = CertificateDer::from(authority.root_der.clone());
    let anchor = webpki::anchor_from_trusted_cert(&root_der).map_err(|error| {
        tracing::warn!(%error, "device root certificate is not a trust anchor");
        DeviceError::Unavailable(CA_UNUSABLE)
    })?;
    let intermediates = if authority.intermediate_der == authority.root_der {
        Vec::new()
    } else {
        vec![CertificateDer::from(authority.intermediate_der.clone())]
    };
    end.verify_for_usage(
        webpki::ALL_VERIFICATION_ALGS,
        &[anchor],
        &intermediates,
        UnixTime::now(),
        KeyUsage::client_auth(),
        None,
        None,
    )
    .map_err(|error| {
        tracing::warn!(%error, "issued device certificate was rejected");
        DeviceError::Unavailable(CA_UNUSABLE)
    })?;
    Ok(())
}

fn package_p12(
    cert_der: &[u8],
    key_der: &[u8],
    authority: &Authority,
    password: &str,
    name: &str,
) -> Result<Vec<u8>, DeviceError> {
    let mut cas = vec![authority.intermediate_der.as_slice()];
    if authority.root_der != authority.intermediate_der {
        cas.push(authority.root_der.as_slice());
    }
    let pfx = p12::PFX::new_with_cas(cert_der, key_der, &cas, password, name).ok_or_else(|| {
        tracing::warn!("device certificate could not be packed");
        DeviceError::Unavailable(CA_UNUSABLE)
    })?;
    Ok(pfx.to_der())
}

pub(super) fn encode_p12(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn subject_matches(left_der: &[u8], right_der: &[u8]) -> bool {
    match (subject_raw(left_der), subject_raw(right_der)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn subject_raw(der: &[u8]) -> Result<Vec<u8>, DeviceError> {
    let (_, cert) =
        X509Certificate::from_der(der).map_err(|_| DeviceError::Unavailable(CA_UNUSABLE))?;
    Ok(cert.subject().as_raw().to_vec())
}

fn distinguished_name(der: &[u8]) -> Result<rcgen::DistinguishedName, DeviceError> {
    let (_, cert) = X509Certificate::from_der(der).map_err(|_| {
        tracing::warn!("device intermediate certificate did not parse");
        DeviceError::Unavailable(CA_UNUSABLE)
    })?;
    let mut name = rcgen::DistinguishedName::new();
    let mut seen = Vec::new();
    for rdn in cert.subject().iter_rdn() {
        let mut attributes = rdn.iter();
        let Some(attribute) = attributes.next() else {
            return Err(DeviceError::Unavailable(CA_UNUSABLE));
        };
        if attributes.next().is_some() {
            tracing::warn!("device issuer name has a multi-valued component");
            return Err(DeviceError::Unavailable(CA_UNUSABLE));
        }
        let arcs = attribute
            .attr_type()
            .iter()
            .ok_or(DeviceError::Unavailable(CA_UNUSABLE))?
            .collect::<Vec<_>>();
        let kind = DnType::from_oid(&arcs);
        if seen.iter().any(|existing| existing == &kind) {
            tracing::warn!("device issuer name repeats an attribute");
            return Err(DeviceError::Unavailable(CA_UNUSABLE));
        }
        seen.push(kind.clone());
        name.push(kind, directory_string(attribute)?);
    }
    Ok(name)
}

fn directory_string(
    attribute: &x509_parser::x509::AttributeTypeAndValue<'_>,
) -> Result<DnValue, DeviceError> {
    let bytes = attribute.as_slice();
    match attribute.attr_value().tag().0 {
        12 => utf8_directory(bytes).map(DnValue::Utf8String),
        19 => printable_directory(bytes),
        22 => ia5_directory(bytes),
        20 => teletex_directory(bytes),
        30 => rcgen::BmpString::from_utf16be(bytes.to_vec())
            .map(DnValue::BmpString)
            .map_err(|error| directory_error(&error)),
        28 => rcgen::UniversalString::from_utf32be(bytes.to_vec())
            .map(DnValue::UniversalString)
            .map_err(|error| directory_error(&error)),
        _ => {
            tracing::warn!("device issuer name uses an unsupported string type");
            Err(DeviceError::Unavailable(CA_UNUSABLE))
        }
    }
}

fn utf8_directory(bytes: &[u8]) -> Result<String, DeviceError> {
    std::str::from_utf8(bytes)
        .map(ToOwned::to_owned)
        .map_err(|_| DeviceError::Unavailable(CA_UNUSABLE))
}

fn printable_directory(bytes: &[u8]) -> Result<DnValue, DeviceError> {
    let text = utf8_directory(bytes)?;
    rcgen::PrintableString::try_from(text)
        .map(DnValue::PrintableString)
        .map_err(|error| directory_error(&error))
}

fn ia5_directory(bytes: &[u8]) -> Result<DnValue, DeviceError> {
    let text = utf8_directory(bytes)?;
    rcgen::Ia5String::try_from(text)
        .map(DnValue::Ia5String)
        .map_err(|error| directory_error(&error))
}

fn teletex_directory(bytes: &[u8]) -> Result<DnValue, DeviceError> {
    let text = utf8_directory(bytes)?;
    rcgen::TeletexString::try_from(text)
        .map(DnValue::TeletexString)
        .map_err(|error| directory_error(&error))
}

fn directory_error(error: &rcgen::Error) -> DeviceError {
    tracing::warn!(%error, "device issuer name could not be rebuilt");
    DeviceError::Unavailable(CA_UNUSABLE)
}

fn pem_certs(bytes: &[u8]) -> Result<Vec<Vec<u8>>, DeviceError> {
    let mut certs = Vec::new();
    let mut rest = bytes;
    loop {
        if rest.iter().all(u8::is_ascii_whitespace) {
            break;
        }
        match x509_parser::pem::parse_x509_pem(rest) {
            Ok((next, pem)) if pem.label == "CERTIFICATE" => {
                certs.push(pem.contents);
                rest = next;
            }
            Ok(_) | Err(_) => {
                tracing::warn!("device certificate authority pem is unusable");
                return Err(DeviceError::Unavailable(CA_UNUSABLE));
            }
        }
    }
    if certs.is_empty() {
        return Err(DeviceError::Unavailable(CA_UNUSABLE));
    }
    Ok(certs)
}

fn one_cert(bytes: &[u8]) -> Result<Vec<u8>, DeviceError> {
    let certs = pem_certs(bytes)?;
    if certs.len() != 1 {
        tracing::warn!("device certificate authority file does not hold one certificate");
        return Err(DeviceError::Unavailable(CA_UNUSABLE));
    }
    certs
        .into_iter()
        .next()
        .ok_or(DeviceError::Unavailable(CA_UNUSABLE))
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, DeviceError> {
    std::fs::read(path).map_err(|error| {
        tracing::warn!(%error, "device certificate authority file is missing");
        DeviceError::Unavailable(CA_UNUSABLE)
    })
}

fn read_text(path: &Path) -> Result<String, DeviceError> {
    let bytes = read_bytes(path)?;
    String::from_utf8(bytes).map_err(|_| {
        tracing::warn!("device certificate authority key is not utf-8");
        DeviceError::Unavailable(CA_UNUSABLE)
    })
}
