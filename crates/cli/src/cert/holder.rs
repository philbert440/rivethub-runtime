use crate::Error;
use rcgen::{CertificateParams, DistinguishedName, KeyPair, SanType};
use std::collections::BTreeSet;
use std::fs::File;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

use super::token::Enrollment;
use super::validate::{Accepted, Name};

const KEY_FILE: &str = "holder.key.pem";
const CERTIFICATE_FILE: &str = "holder.cert.pem";

pub(super) struct LocalHolder {
    directory: PathBuf,
    _lock: File,
    key: KeyPair,
    key_pem: String,
    root_pem: String,
    enrollment: Option<Enrollment>,
    certificate: Option<String>,
    requested: BTreeSet<Name>,
    renew_at: Option<i64>,
    principal: Name,
}

impl LocalHolder {
    pub(super) fn open(
        state: &Path,
        trust_root: &Path,
        enrollment: Option<Enrollment>,
        principal: &str,
        now: OffsetDateTime,
    ) -> Result<Self, Error> {
        let principal = super::validate::principal(principal)?;
        if let Some(enrollment) = &enrollment {
            super::validate::foreign_uri(&enrollment.names, &principal)?;
        }
        let root_pem = super::files::read_external(trust_root)?;
        if enrollment.is_none() && !super::files::certificate_stored(state, CERTIFICATE_FILE)? {
            return Err(Error::PkiInput("first enrollment requires --token-file"));
        }
        let directory = super::files::tls_directory(state)?;
        let lock = super::files::lock(&directory)?;
        let certificate = super::files::read_certificate(&directory, CERTIFICATE_FILE)?;
        if enrollment.is_none() && certificate.is_none() {
            return Err(Error::PkiInput("first enrollment requires --token-file"));
        }
        let (key, key_pem) = load_or_create_key(&directory)?;
        let (requested, renew_at) = if let Some(enrollment) = &enrollment {
            (enrollment.names.iter().cloned().collect(), None)
        } else if let Some(pem) = certificate.as_deref() {
            let accepted = super::validate::establish(pem, &root_pem, &key, &principal, now)?;
            (accepted.names, Some(accepted.renew_at))
        } else {
            return Err(Error::PkiInput("first enrollment requires --token-file"));
        };
        Ok(Self {
            directory,
            _lock: lock,
            key,
            key_pem,
            root_pem,
            enrollment,
            certificate,
            requested,
            renew_at,
            principal,
        })
    }

    pub(super) fn renew_at(&self) -> Option<i64> {
        self.renew_at
    }

    pub(super) fn token(&self) -> Option<&str> {
        self.enrollment
            .as_ref()
            .map(|enrollment| enrollment.token.as_str())
    }

    pub(super) fn csr(&self) -> Result<String, Error> {
        let enrollment = self.enrollment.as_ref().ok_or(Error::PkiInput(
            "renewal does not build a certificate request",
        ))?;
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params.subject_alt_names = enrollment
            .names
            .iter()
            .map(san)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(params.serialize_request(&self.key)?.pem()?)
    }

    pub(super) fn identity(&self) -> Result<(&str, &str), Error> {
        let certificate = self
            .certificate
            .as_deref()
            .ok_or(Error::PkiInput("renewal requires a holder certificate"))?;
        Ok((certificate, self.key_pem.as_str()))
    }

    pub(super) fn install(
        &mut self,
        signed: &stepca::SignedCertificate,
        now: OffsetDateTime,
    ) -> Result<(), Error> {
        let pem = chain(signed)?;
        let Accepted { names, renew_at } = super::validate::accept(
            &pem,
            &self.root_pem,
            &self.key,
            &self.requested,
            &self.principal,
            now,
        )?;
        super::files::publish(&self.directory, CERTIFICATE_FILE, pem.as_bytes())?;
        self.certificate = Some(pem);
        self.requested = names;
        self.renew_at = Some(renew_at);
        self.enrollment = None;
        Ok(())
    }
}

fn load_or_create_key(directory: &Path) -> Result<(KeyPair, String), Error> {
    if super::files::read_certificate(directory, KEY_FILE)?.is_some() {
        let pem = super::files::read_key(directory, KEY_FILE)?;
        let key = KeyPair::from_pem(&pem)?;
        if key.algorithm() == &rcgen::PKCS_ECDSA_P256_SHA256 {
            Ok((key, pem))
        } else {
            Err(Error::PkiInput("holder key must use P-256"))
        }
    } else {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let pem = key.serialize_pem();
        super::files::publish(directory, KEY_FILE, pem.as_bytes())?;
        Ok((key, pem))
    }
}

fn san(name: &Name) -> Result<SanType, Error> {
    match name {
        Name::Dns(value) => Ok(SanType::DnsName(value.as_str().try_into()?)),
        Name::Ip(value) => Ok(SanType::IpAddress(*value)),
        Name::Uri(value) => Ok(SanType::URI(value.as_str().try_into()?)),
    }
}

fn chain(signed: &stepca::SignedCertificate) -> Result<String, Error> {
    let mut pem = String::new();
    push_certificate(&mut pem, &signed.leaf)?;
    push_certificate(&mut pem, &signed.intermediate)?;
    Ok(pem)
}

fn push_certificate(output: &mut String, block: &str) -> Result<(), Error> {
    let trimmed = block.trim();
    if trimmed.contains("-----BEGIN CERTIFICATE-----") && !trimmed.contains("PRIVATE KEY") {
        output.push_str(trimmed);
        output.push('\n');
        Ok(())
    } else {
        Err(Error::PkiInput(
            "step-ca response did not contain a PEM certificate",
        ))
    }
}
