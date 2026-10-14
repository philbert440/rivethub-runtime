use crate::{Error, Pki, files, import, storage};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use std::path::PathBuf;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

impl Pki {
    pub async fn initialize(
        directory: PathBuf,
        root_key_output: PathBuf,
        cancellation: CancellationToken,
    ) -> Result<Self, Error> {
        let epoch = OffsetDateTime::now_utc();
        let started = tokio::time::Instant::now();
        let cancel = cancellation.clone();
        let inner = tokio::task::spawn_blocking(move || {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let directory = files::directory(&directory, true)?;
            let lock = files::lock(&directory)?;
            let parent = root_key_output.parent().ok_or_else(|| {
                crate::error::refused("root key export requires a parent directory")
            })?;
            let parent = files::directory(parent, false)?;
            if parent.starts_with(&directory) {
                return Err(crate::error::refused(
                    "root key export must be outside CA state",
                ));
            }
            let name = root_key_output
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .ok_or_else(|| {
                    crate::error::refused("root key export requires a UTF-8 filename")
                })?;
            let mut connection = storage::initialize(&directory, "1")?;
            let transaction = connection.transaction()?;
            let root_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
            let root_params = parameters("RivetHub root", 1, epoch, 3650);
            let root = root_params.self_signed(&root_key)?;
            let issuer = Issuer::new(root_params, root_key);
            let intermediate_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
            let intermediate = parameters("RivetHub intermediate", 0, epoch, 365)
                .signed_by(&intermediate_key, &issuer)?;
            import::signer(
                root.pem(),
                intermediate.pem(),
                &intermediate_key.serialize_pem(),
                epoch,
            )?;
            files::create(&parent, name, issuer.key().serialize_pem().as_bytes())?;
            std::fs::File::open(&parent)?.sync_all()?;
            drop(issuer);
            files::create(&directory, "root.cert.pem", root.pem().as_bytes())?;
            files::create(
                &directory,
                "intermediate.cert.pem",
                intermediate.pem().as_bytes(),
            )?;
            files::create(
                &directory,
                "intermediate.key.pem",
                intermediate_key.serialize_pem().as_bytes(),
            )?;
            transaction.execute("UPDATE rh_ca_meta SET ready=1 WHERE id=1", [])?;
            transaction.commit()?;
            std::fs::File::open(&directory)?.sync_all()?;
            Self::load(directory, lock, epoch)
        })
        .await??;
        Ok(Self {
            inner: Arc::new(inner),
            cancellation,
            epoch,
            started,
        })
    }
}

fn parameters(name: &str, depth: u8, now: OffsetDateTime, days: i64) -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut subject = DistinguishedName::new();
    subject.push(DnType::CommonName, name);
    params.distinguished_name = subject;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(depth));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = now - time::Duration::minutes(5);
    params.not_after = now + time::Duration::days(days);
    params
}
