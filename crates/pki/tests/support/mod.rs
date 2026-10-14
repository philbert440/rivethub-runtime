use kernel::ca::{CsrPem, Principal};
use pki::{ImportLayout, Pki};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, Issuer, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls_pki_types::pem::PemObject;
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

pub(crate) type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub(crate) struct Fixture {
    pub(crate) directory: tempfile::TempDir,
    pub(crate) source: PathBuf,
    pub(crate) state: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Result<Self> {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/pki-tests");
        fs::create_dir_all(&base)?;
        let directory = tempfile::tempdir_in(fs::canonicalize(base)?)?;
        let source = directory.path().join("legacy");
        let state = directory.path().join("state");
        fs::create_dir_all(source.join("root/certs"))?;
        fs::create_dir_all(source.join("intermediate/certs"))?;
        fs::create_dir_all(source.join("intermediate/private"))?;
        let root_key = KeyPair::generate()?;
        let root_params = ca_params("RivetHub test root", 1);
        let root = root_params.self_signed(&root_key)?;
        let issuer = Issuer::new(root_params, root_key);
        let intermediate_key = KeyPair::generate()?;
        let intermediate =
            ca_params("RivetHub test intermediate", 0).signed_by(&intermediate_key, &issuer)?;
        fs::write(source.join("root/certs/ca.cert.pem"), root.pem())?;
        fs::write(
            source.join("intermediate/certs/intermediate.cert.pem"),
            intermediate.pem(),
        )?;
        let mut key_file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(source.join("intermediate/private/intermediate.key.pem"))?;
        key_file.write_all(intermediate_key.serialize_pem().as_bytes())?;
        fs::write(
            source.join("intermediate/index.txt"),
            "R\t351001000000Z\t251001000000Z,keyCompromise\t1000\tunknown\t/CN=legacy-node\nV\t351001000000Z\t\t1001\tunknown\t/CN=legacy-device\n",
        )?;
        fs::write(source.join("intermediate/serial"), "1002\n")?;
        Ok(Self {
            directory,
            source,
            state,
        })
    }

    pub(crate) async fn import(&self) -> Result<Pki> {
        Ok(Pki::import(
            self.source.clone(),
            self.state.clone(),
            ImportLayout::openssl(),
            CancellationToken::new(),
        )
        .await?)
    }
}

fn ca_params(name: &str, depth: u8) -> CertificateParams {
    let mut distinguished_name = DistinguishedName::new();
    distinguished_name.push(DnType::CommonName, name);
    let mut params = CertificateParams::default();
    params.distinguished_name = distinguished_name;
    if depth == 0 {
        params.key_identifier_method = rcgen::KeyIdMethod::PreSpecified(vec![7; 20]);
    }
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(depth));
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    params.not_before = OffsetDateTime::now_utc() - time::Duration::days(1);
    params.not_after = OffsetDateTime::now_utc() + time::Duration::days(365);
    params
}

pub(crate) fn node() -> Principal {
    Principal::Node {
        node: "node-a".to_owned(),
    }
}

pub(crate) fn csr(names: &[&str]) -> Result<CsrPem> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::default();
    params.subject_alt_names = names
        .iter()
        .map(|name| Ok(SanType::URI((*name).try_into()?)))
        .collect::<Result<Vec<_>>>()?;
    Ok(CsrPem(params.serialize_request(&key)?.pem()?))
}

pub(crate) fn certificate(pem: &str) -> Result<Vec<u8>> {
    Ok(rustls_pki_types::CertificateDer::from_pem_slice(pem.as_bytes())?.to_vec())
}
