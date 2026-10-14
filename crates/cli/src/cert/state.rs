use crate::Error;
use kernel::ca::EnrollmentToken;
use pki::files;
use rcgen::{CertificateParams, DistinguishedName, KeyPair, SanType};
use rustls_pki_types::pem::PemObject;
use std::fs::File;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use wire::ca::SignResponse;

pub(super) struct Holder {
    directory: PathBuf,
    _lock: File,
    pub(super) key: KeyPair,
    pub(super) root: String,
    pub(super) token: Option<EnrollmentToken>,
    pub(super) certificate: Option<String>,
    pub(super) csr: String,
    pub(super) renew_at: Option<i64>,
    principal: String,
}

impl Holder {
    pub(super) fn open(
        directory: &Path,
        root_path: &Path,
        token_path: Option<&Path>,
        principal: &str,
        now: OffsetDateTime,
    ) -> Result<Self, Error> {
        let directory = files::directory(directory, true)?;
        let lock = files::lock(&directory)?;
        let root = read_path(root_path)?;
        pki::certificate(&root)?;
        let key_path = directory.join("holder.key.pem");
        let key = if key_path.try_exists()? {
            let key_path = files::contained(&directory, Path::new("holder.key.pem"))?;
            let metadata = std::fs::metadata(key_path)?;
            if metadata.permissions().mode() & 0o777 != 0o600 || metadata.nlink() != 1 {
                return Err(Error::PkiInput(
                    "holder key must have mode 0600 and one hard link",
                ));
            }
            KeyPair::from_pem(&files::read(&directory, Path::new("holder.key.pem"))?)?
        } else {
            let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
            files::create(&directory, "holder.key.pem", key.serialize_pem().as_bytes())?;
            File::open(&directory)?.sync_all()?;
            key
        };
        if key.algorithm() != &rcgen::PKCS_ECDSA_P256_SHA256 {
            return Err(Error::PkiInput("holder key must use P-256"));
        }
        let (certificate, renew_at) = if directory.join("holder.cert.pem").try_exists()? {
            let pem = files::read(&directory, Path::new("holder.cert.pem"))?;
            let ders = rustls_pki_types::CertificateDer::pem_slice_iter(pem.as_bytes())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| Error::PkiInput("invalid holder certificate PEM"))?;
            let renewal = super::validate::certificate(&ders, &root, &key, principal, now)?;
            (Some(pem), Some(renewal))
        } else {
            (None, None)
        };
        let token = match (token_path, certificate.is_some()) {
            (Some(_), true) => {
                return Err(Error::PkiInput(
                    "an existing certificate must renew with mTLS, without a token",
                ));
            }
            (Some(path), false) => {
                let text = read_path(path)?;
                let token = text.trim();
                if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(Error::PkiInput("invalid enrollment token file"));
                }
                Some(EnrollmentToken(token.to_owned()))
            }
            (None, true) => None,
            (None, false) => return Err(Error::PkiInput("first enrollment requires --token-file")),
        };
        Ok(Self {
            directory,
            _lock: lock,
            key,
            root,
            token,
            certificate,
            csr: String::new(),
            renew_at,
            principal: principal.to_owned(),
        })
    }

    pub(super) fn prepare(&mut self) -> Result<(), Error> {
        let mut params = CertificateParams::default();
        params.distinguished_name = DistinguishedName::new();
        params.subject_alt_names = vec![SanType::URI(self.principal.as_str().try_into()?)];
        self.csr = params.serialize_request(&self.key)?.pem()?;
        Ok(())
    }

    pub(super) fn install(
        &mut self,
        response: SignResponse,
        now: OffsetDateTime,
    ) -> Result<(), Error> {
        let leaf = rustls_pki_types::CertificateDer::from(pki::certificate(&response.leaf)?);
        let mut certificates = vec![leaf];
        for pem in &response.chain {
            certificates.push(pki::certificate(pem)?.into());
        }
        let renewal = super::validate::certificate(
            &certificates,
            &self.root,
            &self.key,
            &self.principal,
            now,
        )?;
        if renewal <= now.unix_timestamp() {
            return Err(Error::PkiInput(
                "CA certificate has already reached its renewal deadline",
            ));
        }
        let mut pem = response.leaf;
        for certificate in response.chain {
            pem.push('\n');
            pem.push_str(&certificate);
        }
        files::replace_public(&self.directory, "holder.cert.pem", pem.as_bytes())?;
        self.certificate = Some(pem);
        self.renew_at = Some(renewal);
        self.token = None;
        Ok(())
    }
}

fn read_path(path: &Path) -> Result<String, Error> {
    let path = std::path::absolute(path)?;
    let parent = path
        .parent()
        .ok_or(Error::PkiInput("PKI input requires a parent path"))?;
    let directory = files::directory(parent, false)?;
    let name = path
        .file_name()
        .ok_or(Error::PkiInput("PKI input requires a filename"))?;
    Ok(files::read(&directory, Path::new(name))?)
}
