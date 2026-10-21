use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::Path;

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};

use super::Error;
use super::recording::ClientCertificate;

pub(super) struct Certificates {
    pub(super) root: reqwest::Certificate,
    pub(super) clients: BTreeMap<ClientCertificate, reqwest::Identity>,
}

pub(super) fn generate(directory: &Path, remote: IpAddr) -> Result<Certificates, Error> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(DnType::CommonName, "Replay Test CA");
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let root = params.self_signed(&key)?;
    let issuer = rcgen::Issuer::from_params(&params, &key);
    let server_key = KeyPair::generate()?;
    let mut params = CertificateParams::new(vec![
        "localhost".into(),
        "127.0.0.1".into(),
        remote.to_string(),
    ])?;
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server = params.signed_by(&server_key, &issuer)?;
    std::fs::write(directory.join("server.pem"), server.pem())?;
    std::fs::write(directory.join("server.key"), server_key.serialize_pem())?;
    std::fs::write(directory.join("ca.pem"), root.pem())?;
    let mut clients = BTreeMap::new();
    for kind in [
        ClientCertificate::DeviceTest,
        ClientCertificate::DeviceCnOnly,
        ClientCertificate::OuClient,
        ClientCertificate::Node,
        ClientCertificate::DeviceNodb,
        ClientCertificate::DeviceStranger,
        ClientCertificate::WrongCaDevice,
        ClientCertificate::DeviceServerauth,
    ] {
        clients.insert(kind, client(kind, &issuer)?);
    }
    Ok(Certificates {
        root: reqwest::Certificate::from_pem(root.pem().as_bytes())?,
        clients,
    })
}

fn client(
    kind: ClientCertificate,
    issuer: &rcgen::Issuer<'_, &KeyPair>,
) -> Result<reqwest::Identity, Error> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::default();
    let name = match kind {
        ClientCertificate::DeviceTest
        | ClientCertificate::WrongCaDevice
        | ClientCertificate::DeviceServerauth => "device:test",
        ClientCertificate::DeviceCnOnly => "device:cnonly",
        ClientCertificate::OuClient => "hub-client",
        ClientCertificate::Node => "golden-node.mesh",
        ClientCertificate::DeviceNodb => "device:nodb",
        ClientCertificate::DeviceStranger => "device:stranger",
    };
    params.distinguished_name.push(DnType::CommonName, name);
    if !matches!(
        kind,
        ClientCertificate::DeviceCnOnly | ClientCertificate::Node
    ) {
        params
            .distinguished_name
            .push(DnType::OrganizationalUnitName, "client");
    }
    params.extended_key_usages = vec![if kind == ClientCertificate::DeviceServerauth {
        ExtendedKeyUsagePurpose::ServerAuth
    } else {
        ExtendedKeyUsagePurpose::ClientAuth
    }];
    let certificate = if kind == ClientCertificate::WrongCaDevice {
        let other_key = KeyPair::generate()?;
        let mut other = CertificateParams::default();
        other.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        other
            .distinguished_name
            .push(DnType::CommonName, "Untrusted Test CA");
        params.signed_by(&key, &rcgen::Issuer::from_params(&other, &other_key))?
    } else {
        params.signed_by(&key, issuer)?
    };
    Ok(reqwest::Identity::from_pem(
        format!("{}{}", certificate.pem(), key.serialize_pem()).as_bytes(),
    )?)
}
