use crate::{error::Error, files, storage};
use chrono::NaiveDateTime;
use rcgen::{Issuer, KeyPair, PublicKeyData};
use rusqlite::params;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use x509_parser::prelude::FromDer;

#[derive(Debug, Clone)]
pub struct ImportLayout {
    pub root_certificate: PathBuf,
    pub intermediate_certificate: PathBuf,
    pub intermediate_key: PathBuf,
    pub index: PathBuf,
    pub serial: PathBuf,
}

impl ImportLayout {
    pub fn openssl() -> Self {
        Self {
            root_certificate: "root/certs/ca.cert.pem".into(),
            intermediate_certificate: "intermediate/certs/intermediate.cert.pem".into(),
            intermediate_key: "intermediate/private/intermediate.key.pem".into(),
            index: "intermediate/index.txt".into(),
            serial: "intermediate/serial".into(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Signer {
    pub(crate) issuer: Issuer<'static, KeyPair>,
    pub(crate) chain: Vec<String>,
    pub(crate) key_identifier: rcgen::KeyIdMethod,
    pub(crate) not_before: OffsetDateTime,
    pub(crate) not_after: OffsetDateTime,
}

fn import_error(error: &str) -> Error {
    Error::Import(error.to_string())
}

pub(crate) fn certificate(pem: &str) -> Result<Vec<u8>, Error> {
    let mut reader = pem.as_bytes();
    let Some((rustls_pki_types::pem::SectionKind::Certificate, der)) =
        rustls_pki_types::pem::from_buf(&mut reader)?
    else {
        return Err(import_error("expected one certificate"));
    };
    if !reader.iter().all(u8::is_ascii_whitespace) {
        return Err(import_error("trailing certificate data"));
    }
    Ok(der)
}

pub(crate) fn signer(
    root: String,
    intermediate: String,
    key: &str,
    now: OffsetDateTime,
) -> Result<Signer, Error> {
    let root_der = certificate(&root)?;
    let intermediate_der = certificate(&intermediate)?;
    let (root_rest, root_cert) = x509_parser::certificate::X509Certificate::from_der(&root_der)
        .map_err(|error| import_error(&error.to_string()))?;
    let (intermediate_rest, intermediate_cert) =
        x509_parser::certificate::X509Certificate::from_der(&intermediate_der)
            .map_err(|error| import_error(&error.to_string()))?;
    if !root_rest.is_empty() || !intermediate_rest.is_empty() {
        return Err(import_error("trailing CA DER"));
    }
    validate_ca(&root_cert, now)?;
    validate_ca(&intermediate_cert, now)?;
    if root_cert.subject() != root_cert.issuer() {
        return Err(import_error("root must be self-issued"));
    }
    if intermediate_cert.issuer() != root_cert.subject() {
        return Err(import_error("intermediate issuer must match root subject"));
    }
    root_cert
        .verify_signature(None)
        .map_err(|error| import_error(&error.to_string()))?;
    intermediate_cert
        .verify_signature(Some(root_cert.public_key()))
        .map_err(|error| import_error(&error.to_string()))?;
    let key = KeyPair::from_pem(key)?;
    if key.algorithm() != &rcgen::PKCS_ECDSA_P256_SHA256
        || key.subject_public_key_info() != intermediate_cert.public_key().raw
    {
        return Err(import_error(
            "intermediate must have a matching ECDSA P-256 key",
        ));
    }
    let not_before = OffsetDateTime::from_unix_timestamp(
        intermediate_cert
            .validity()
            .not_before
            .timestamp()
            .max(root_cert.validity().not_before.timestamp()),
    )?;
    let not_after = OffsetDateTime::from_unix_timestamp(
        intermediate_cert
            .validity()
            .not_after
            .timestamp()
            .min(root_cert.validity().not_after.timestamp()),
    )?;
    let key_identifier = intermediate_cert
        .extensions()
        .iter()
        .find_map(|extension| {
            if let x509_parser::extensions::ParsedExtension::SubjectKeyIdentifier(identifier) =
                extension.parsed_extension()
            {
                Some(rcgen::KeyIdMethod::PreSpecified(identifier.0.to_vec()))
            } else {
                None
            }
        })
        .ok_or_else(|| import_error("intermediate requires a subject key identifier"))?;
    Ok(Signer {
        key_identifier,
        issuer: Issuer::from_ca_cert_pem(&intermediate, key)?,
        chain: vec![intermediate, root],
        not_before,
        not_after,
    })
}

fn validate_ca(
    cert: &x509_parser::certificate::X509Certificate<'_>,
    now: OffsetDateTime,
) -> Result<(), Error> {
    if !cert
        .basic_constraints()
        .map_err(|error| import_error(&error.to_string()))?
        .is_some_and(|constraint| constraint.value.ca)
        || !cert
            .key_usage()
            .map_err(|error| import_error(&error.to_string()))?
            .is_some_and(|usage| usage.value.key_cert_sign() && usage.value.crl_sign())
    {
        return Err(import_error(
            "CA must authorize certificate and CRL signing",
        ));
    }
    if now.unix_timestamp() < cert.validity().not_before.timestamp()
        || now.unix_timestamp() >= cert.validity().not_after.timestamp()
    {
        return Err(import_error("CA is not currently valid"));
    }
    Ok(())
}

pub(crate) fn run(
    source: &Path,
    destination: &Path,
    layout: &ImportLayout,
    now: OffsetDateTime,
) -> Result<(), Error> {
    let source = files::directory(source, false)?;
    let root = files::read(&source, &layout.root_certificate)?;
    let intermediate = files::read(&source, &layout.intermediate_certificate)?;
    let key = files::read(&source, &layout.intermediate_key)?;
    let imported = signer(root.clone(), intermediate.clone(), &key, now)?;
    let index = files::read(&source, &layout.index)?;
    let serial_text = files::read(&source, &layout.serial)?;
    let next_serial = storage::serial(serial_text.trim())?;
    let mut connection = storage::initialize(destination, &next_serial.to_str_radix(16))?;
    let transaction = connection.transaction()?;
    for line in index.lines() {
        let columns = line.split('\t').collect::<Vec<_>>();
        let [status, expires, revoked, serial, _file, _subject] = columns.as_slice() else {
            return Err(import_error(
                "index row must have six tab-separated columns",
            ));
        };
        timestamp(expires)?;
        let serial = storage::serial(serial)?;
        if serial >= next_serial {
            return Err(import_error("index serial is not below the next serial"));
        }
        let revoked_at = match (*status, revoked.is_empty()) {
            ("V" | "E", true) => None,
            ("R", false) => Some(timestamp(
                revoked
                    .split(',')
                    .next()
                    .ok_or_else(|| import_error("missing revocation date"))?,
            )?),
            _ => return Err(import_error("invalid index status or revocation date")),
        };
        transaction.execute(
            "INSERT INTO rh_ca_certificates VALUES(?1, NULL, ?2)",
            params![serial.to_str_radix(16), revoked_at],
        )?;
    }
    files::create(destination, "root.cert.pem", root.as_bytes())?;
    files::create(
        destination,
        "intermediate.cert.pem",
        intermediate.as_bytes(),
    )?;
    files::create(
        destination,
        "intermediate.key.pem",
        imported.issuer.key().serialize_pem().as_bytes(),
    )?;
    transaction.execute("UPDATE rh_ca_meta SET ready=1 WHERE id=1", [])?;
    transaction.commit()?;
    std::fs::File::open(destination)?.sync_all()?;
    Ok(())
}

fn timestamp(text: &str) -> Result<i64, Error> {
    let format = match text.len() {
        13 => "%y%m%d%H%M%SZ",
        15 => "%Y%m%d%H%M%SZ",
        _ => return Err(import_error("invalid index timestamp length")),
    };
    Ok(NaiveDateTime::parse_from_str(text, format)
        .map_err(|error| import_error(&error.to_string()))?
        .and_utc()
        .timestamp())
}
