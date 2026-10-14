use std::path::PathBuf;
use std::time::Duration;

use gateway::mesh::ChannelSettings;
use gateway::tls::TlsPaths;
use rcgen::{
    BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
};

pub type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug)]
pub struct Material {
    _directory: tempfile::TempDir,
    pub settings: ChannelSettings,
    pub root: reqwest::Certificate,
    pub peer: reqwest::Identity,
}

impl Material {
    pub fn new() -> TestResult<Self> {
        let directory = tempfile::tempdir()?;
        let root_key = KeyPair::generate()?;
        let mut root_params = CertificateParams::new(vec!["mesh.example.test".to_owned()])?;
        root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        root_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let root = root_params.self_signed(&root_key)?;
        let server_key = KeyPair::generate()?;
        let mut server_params =
            CertificateParams::new(vec!["127.0.0.1".to_owned(), "localhost".to_owned()])?;
        server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let server = server_params.signed_by(
            &server_key,
            &rcgen::Issuer::from_params(&root_params, &root_key),
        )?;
        let peer_key = KeyPair::generate()?;
        let mut peer_params = CertificateParams::new(vec!["peer.example.test".to_owned()])?;
        peer_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
        let peer = peer_params.signed_by(
            &peer_key,
            &rcgen::Issuer::from_params(&root_params, &root_key),
        )?;
        let write = |name: &str, value: String| -> std::io::Result<PathBuf> {
            let path = directory.path().join(name);
            std::fs::write(&path, value)?;
            Ok(path)
        };
        let tls = TlsPaths {
            crl: None,
            cert: write("node.crt", server.pem())?,
            key: write("node.key", server_key.serialize_pem())?,
            ca: Some(write("ca.crt", root.pem())?),
        };
        Ok(Self {
            _directory: directory,
            settings: ChannelSettings {
                host: "127.0.0.1".to_owned(),
                agent_channel_port: 0,
                tls,
                request_timeout: Duration::from_secs(30),
            },
            root: reqwest::Certificate::from_pem(root.pem().as_bytes())?,
            peer: reqwest::Identity::from_pem(
                format!("{}\n{}", peer.pem(), peer_key.serialize_pem()).as_bytes(),
            )?,
        })
    }

    pub fn client(&self, peer: Option<reqwest::Identity>) -> TestResult<reqwest::Client> {
        let builder = reqwest::Client::builder()
            .no_proxy()
            .add_root_certificate(self.root.clone())
            .timeout(Duration::from_secs(5));
        let builder = match peer {
            Some(peer) => builder.identity(peer),
            None => builder,
        };
        Ok(builder.build()?)
    }
}
