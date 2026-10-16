use std::sync::OnceLock;

use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::DistinguishedName;
use rcgen::DnType;
use rcgen::ExtendedKeyUsagePurpose;
use rcgen::IsCa;
use rcgen::Issuer;
use rcgen::KeyPair;
use rcgen::KeyUsagePurpose;
use tasks::MeshIdentity;

static SAMPLE: OnceLock<MeshIdentity> = OnceLock::new();

pub(crate) struct Authority {
    params: CertificateParams,
    pub(crate) certificate: rcgen::Certificate,
    pub(crate) key: KeyPair,
    pub(crate) pem: String,
}

pub(crate) struct Leaf {
    pub(crate) certificate: rcgen::Certificate,
    pub(crate) key: KeyPair,
    pub(crate) pem: String,
    pub(crate) key_pem: String,
}

pub(crate) struct Material {
    pub(crate) ca: Authority,
    pub(crate) server: Leaf,
    pub(crate) client: Leaf,
}

pub(crate) fn sample() -> MeshIdentity {
    SAMPLE.get_or_init(sample_identity).clone()
}

fn sample_identity() -> MeshIdentity {
    let material = mint("node.example.test");
    material.identity(&material.ca.pem)
}

pub(crate) fn mint(name: &str) -> Material {
    let ca = authority(name);
    let (server, client) = signed(&ca, name);
    Material { ca, server, client }
}

fn signed(ca: &Authority, name: &str) -> (Leaf, Leaf) {
    let issuer = Issuer::from_params(&ca.params, &ca.key);
    let server = leaf("127.0.0.1", ExtendedKeyUsagePurpose::ServerAuth, &issuer);
    let client = leaf(name, ExtendedKeyUsagePurpose::ClientAuth, &issuer);
    (server, client)
}

fn authority(name: &str) -> Authority {
    let key = KeyPair::generate().expect("mesh ca key");
    let mut params = CertificateParams::new(vec![name.to_owned()]).expect("mesh ca name");
    let mut name_dn = DistinguishedName::new();
    name_dn.push(DnType::CommonName, name);
    params.distinguished_name = name_dn;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    dated(&mut params);
    let certificate = params.self_signed(&key).expect("mesh ca");
    let pem = certificate.pem();
    Authority {
        params,
        certificate,
        key,
        pem,
    }
}

fn leaf(name: &str, purpose: ExtendedKeyUsagePurpose, issuer: &Issuer<'_, &KeyPair>) -> Leaf {
    let key = KeyPair::generate().expect("mesh leaf key");
    let mut params = CertificateParams::new(vec![name.to_owned()]).expect("mesh leaf name");
    params.extended_key_usages = vec![purpose];
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    dated(&mut params);
    let certificate = params.signed_by(&key, issuer).expect("mesh leaf");
    let pem = certificate.pem();
    let key_pem = key.serialize_pem();
    Leaf {
        certificate,
        key,
        pem,
        key_pem,
    }
}

fn dated(params: &mut CertificateParams) {
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    params.not_after = rcgen::date_time_ymd(2027, 12, 31);
}

impl Material {
    pub(crate) fn identity(&self, ca_pem: &str) -> MeshIdentity {
        MeshIdentity::from_pem(&self.client.pem, &self.client.key_pem, ca_pem)
            .expect("mesh identity")
    }
}
