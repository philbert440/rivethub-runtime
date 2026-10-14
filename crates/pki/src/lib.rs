mod error;
pub mod files;
mod import;
mod initialize;
mod policy;
mod storage;

pub use crate::error::Error;
use crate::error::refused;
pub use crate::import::{ImportLayout, certificate};
use async_trait::async_trait;
use kernel::ca::{
    CaError, CertChainPem, CertificateAuthority, CrlPem, CsrPem, EnrollmentToken, Principal,
    SignAuthority,
};
use rcgen::{CertificateRevocationListParams, SerialNumber};
use rusqlite::params;
use std::path::PathBuf;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct Pki {
    inner: Arc<State>,
    cancellation: CancellationToken,
    epoch: OffsetDateTime,
    started: tokio::time::Instant,
}

#[derive(Debug)]
struct State {
    directory: PathBuf,
    signer: import::Signer,
    publication: Option<Publication>,
    _lock: std::fs::File,
}

#[derive(Debug)]
struct Publication {
    directory: PathBuf,
    name: String,
}

impl Pki {
    pub async fn import(
        source: PathBuf,
        directory: PathBuf,
        layout: ImportLayout,
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
            import::run(&source, &directory, &layout, epoch)?;
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

    pub async fn open(directory: PathBuf, cancellation: CancellationToken) -> Result<Self, Error> {
        let epoch = OffsetDateTime::now_utc();
        let started = tokio::time::Instant::now();
        let cancel = cancellation.clone();
        let inner = tokio::task::spawn_blocking(move || {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            let directory = files::directory(&directory, false)?;
            let lock = files::lock(&directory)?;
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

    fn load(directory: PathBuf, lock: std::fs::File, now: OffsetDateTime) -> Result<State, Error> {
        let connection = storage::connect(&directory)?;
        let ready: bool =
            connection.query_row("SELECT ready FROM rh_ca_meta WHERE id=1", [], |row| {
                row.get(0)
            })?;
        if !ready {
            return Err(Error::Import("CA import is incomplete".to_owned()));
        }
        let root = files::read(&directory, std::path::Path::new("root.cert.pem"))?;
        let intermediate = files::read(&directory, std::path::Path::new("intermediate.cert.pem"))?;
        let key = files::private_key(&directory)?;
        let signer = import::signer(root, intermediate, &key, now)?;
        let state = State {
            directory,
            signer,
            publication: None,
            _lock: lock,
        };
        state.current_crl(now)?;
        Ok(state)
    }

    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&State, OffsetDateTime) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, CaError> {
        if self.cancellation.is_cancelled() {
            return Err(Error::Cancelled.into());
        }
        let state = self.inner.clone();
        let cancel = self.cancellation.clone();
        let elapsed = time::Duration::try_from(self.started.elapsed())
            .map_err(|error| CaError::Unavailable(Box::new(error)))?;
        let now = self.epoch + elapsed;
        tokio::task::spawn_blocking(move || {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            operation(&state, now)
        })
        .await
        .map_err(Error::from)?
        .map_err(CaError::from)
    }

    pub async fn with_crl_publication(mut self, destination: PathBuf) -> Result<Self, CaError> {
        let publication = self
            .run(move |state, _now| {
                let parent = destination
                    .parent()
                    .ok_or_else(|| refused("CRL destination needs a parent directory"))?;
                let directory = files::directory(parent, false)?;
                if directory.starts_with(&state.directory) {
                    return Err(refused("public CRL destination must be outside CA state"));
                }
                let name = destination
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .ok_or_else(|| refused("CRL destination needs a UTF-8 filename"))?
                    .to_owned();
                let path = directory.join(&name);
                if path.try_exists()? {
                    files::contained(&directory, std::path::Path::new(&name))?;
                }
                Ok(Publication { directory, name })
            })
            .await?;
        Arc::get_mut(&mut self.inner)
            .ok_or_else(|| {
                CaError::Refused("configure CRL publication before sharing the CA".to_owned())
            })?
            .publication = Some(publication);
        self.crl().await?;
        Ok(self)
    }

    pub async fn revoke_serial(&self, serial: String) -> Result<(), CaError> {
        self.run(move |state, now| {
            let serial = storage::serial(&serial)?.to_str_radix(16);
            let connection = storage::connect(&state.directory)?;
            let count = connection.execute(
                "UPDATE rh_ca_certificates SET revoked=COALESCE(revoked, ?2) WHERE serial=?1",
                params![serial, now.unix_timestamp()],
            )?;
            if count == 0 {
                return Err(refused("unknown certificate serial"));
            }
            state.current_crl(now)?;
            Ok(())
        })
        .await
    }
}

#[async_trait]
impl CertificateAuthority for Pki {
    async fn issue_token(&self, principal: Principal) -> Result<EnrollmentToken, CaError> {
        self.run(move |state, now| {
            let encoded = serde_json::to_string(&principal)?;
            let principal = principal.spiffe_uri()?;
            let mut connection = storage::connect(&state.directory)?;
            storage::rate(&mut connection, "token", now.unix_timestamp())?;
            let transaction =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            storage::permitted(&transaction, &principal)?;
            transaction.execute(
                "DELETE FROM rh_ca_tokens WHERE expires <= ?1",
                [now.unix_timestamp()],
            )?;
            let mut bytes = [0_u8; 32];
            getrandom::fill(&mut bytes).map_err(Error::Entropy)?;
            let token = hex::encode(bytes);
            let digest = storage::digest(&token);
            transaction.execute(
                "INSERT INTO rh_ca_tokens VALUES(?1, ?2, ?3)",
                params![
                    digest.as_slice(),
                    encoded,
                    (now + time::Duration::minutes(15)).unix_timestamp()
                ],
            )?;
            transaction.commit()?;
            Ok(EnrollmentToken(token))
        })
        .await
    }

    async fn sign(&self, csr: CsrPem, authority: SignAuthority) -> Result<CertChainPem, CaError> {
        self.run(move |state, now| {
            let mut connection = storage::connect(&state.directory)?;
            storage::rate(&mut connection, "sign", now.unix_timestamp())?;
            let transaction =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let principal = match &authority {
                SignAuthority::Token(token) => {
                    storage::authorize_token(&transaction, &token.0, now.unix_timestamp())?
                }
                SignAuthority::Renewal(principal) => principal.clone(),
            };
            let lifetime = policy::lifetime(&principal);
            let principal = principal.spiffe_uri()?;
            storage::permitted(&transaction, &principal)?;
            let mut request = policy::request(&csr.0, &principal)?;
            request.params.not_before = now;
            request.params.not_after = now + lifetime;
            if now < state.signer.not_before || request.params.not_after > state.signer.not_after {
                return Err(refused(
                    "issuer validity cannot cover the requested lifetime",
                ));
            }
            let serial = storage::next_serial(&transaction)?;
            request.params.serial_number = Some(serial.clone());
            let certificate = request.signed_by(&state.signer.issuer)?;
            verify_leaf(certificate.der(), &state.signer.chain, now)?;
            transaction.execute(
                "INSERT INTO rh_ca_certificates VALUES(?1, ?2, NULL)",
                params![
                    num_bigint::BigUint::from_bytes_be(&serial.to_bytes()).to_str_radix(16),
                    principal
                ],
            )?;
            if let SignAuthority::Token(token) = authority {
                transaction.execute(
                    "DELETE FROM rh_ca_tokens WHERE digest=?1",
                    [storage::digest(&token.0).as_slice()],
                )?;
            }
            transaction.commit()?;
            Ok(CertChainPem {
                leaf: certificate.pem(),
                chain: state.signer.chain.clone(),
            })
        })
        .await
    }

    async fn chain(&self) -> Result<Vec<String>, CaError> {
        self.run(|state, _now| Ok(state.signer.chain.clone())).await
    }

    async fn crl(&self) -> Result<CrlPem, CaError> {
        self.run(State::current_crl).await
    }

    async fn revoke(&self, principal: Principal) -> Result<(), CaError> {
        self.run(move |state, now| {
            let encoded = serde_json::to_string(&principal)?;
            let principal = principal.spiffe_uri()?;
            let mut connection = storage::connect(&state.directory)?;
            let transaction =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            transaction.execute(
                "INSERT INTO rh_ca_blocked VALUES(?1) ON CONFLICT DO NOTHING",
                [&principal],
            )?;
            transaction.execute(
                "UPDATE rh_ca_certificates SET revoked=COALESCE(revoked, ?2) WHERE principal=?1",
                params![principal, now.unix_timestamp()],
            )?;
            transaction.execute("DELETE FROM rh_ca_tokens WHERE principal=?1", [&encoded])?;
            transaction.commit()?;
            state.current_crl(now)?;
            Ok(())
        })
        .await
    }
}

impl State {
    fn current_crl(&self, now: OffsetDateTime) -> Result<CrlPem, Error> {
        if now < self.signer.not_before || now >= self.signer.not_after {
            return Err(refused("issuer is outside its validity period"));
        }
        let mut connection = storage::connect(&self.directory)?;
        let transaction =
            connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE rh_ca_meta SET crl_number=crl_number+1 WHERE id=1",
            [],
        )?;
        let number: u64 =
            transaction.query_row("SELECT crl_number FROM rh_ca_meta WHERE id=1", [], |row| {
                row.get(0)
            })?;
        let crl = CertificateRevocationListParams {
            this_update: now,
            next_update: (now + time::Duration::days(1)).min(self.signer.not_after),
            crl_number: SerialNumber::from(number),
            issuing_distribution_point: None,
            revoked_certs: storage::revoked(&transaction)?,
            key_identifier_method: self.signer.key_identifier.clone(),
        }
        .signed_by(&self.signer.issuer)?
        .pem()?;
        files::replace_public(&self.directory, "intermediate.crl.pem", crl.as_bytes())?;
        if let Some(publication) = &self.publication {
            files::replace_public(&publication.directory, &publication.name, crl.as_bytes())?;
        }
        transaction.commit()?;
        Ok(CrlPem(crl))
    }
}

fn verify_leaf(leaf: &[u8], chain: &[String], now: OffsetDateTime) -> Result<(), Error> {
    let [intermediate, root] = chain else {
        return Err(Error::Import("invalid chain length".to_owned()));
    };
    let intermediate = import::certificate(intermediate)?.into();
    let root = import::certificate(root)?.into();
    let leaf = leaf.into();
    let anchor = webpki::anchor_from_trusted_cert(&root)
        .map_err(|error| Error::Import(error.to_string()))?;
    let timestamp =
        u64::try_from(now.unix_timestamp()).map_err(|error| Error::Import(error.to_string()))?;
    webpki::EndEntityCert::try_from(&leaf)
        .map_err(|error| Error::Import(error.to_string()))?
        .verify_for_usage(
            webpki::ALL_VERIFICATION_ALGS,
            &[anchor],
            &[intermediate],
            rustls_pki_types::UnixTime::since_unix_epoch(std::time::Duration::from_secs(timestamp)),
            webpki::KeyUsage::client_auth(),
            None,
            None,
        )
        .map_err(|error| Error::Import(error.to_string()))?;
    Ok(())
}
