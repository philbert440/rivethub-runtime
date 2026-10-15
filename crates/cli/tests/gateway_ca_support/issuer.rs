use std::collections::BTreeMap;

use kernel::ca::{
    CaError, CertChainPem, CertificateAuthority, CrlPem, CsrPem, EnrollmentToken, Principal,
    SignAuthority,
};
use parking_lot::Mutex;
use rcgen::{
    BasicConstraints, CertificateParams, CertificateRevocationListParams,
    CertificateSigningRequestParams, ExtendedKeyUsagePurpose, IsCa, Issuer as CaIssuer,
    KeyIdMethod, KeyPair, KeyUsagePurpose, SanType, SerialNumber,
};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;
use x509_parser::prelude::FromDer;

pub(crate) struct Generated {
    pub(crate) issuer: TestAuthority,
    pub(crate) intermediate: String,
    pub(crate) intermediate_key: String,
    pub(crate) root: String,
}

#[derive(Debug)]
pub(crate) struct TestAuthority {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    next: u64,
    tokens: BTreeMap<String, String>,
    issued: BTreeMap<String, Vec<Vec<u8>>>,
    revoked: Vec<Vec<u8>>,
    intermediate_pem: String,
    intermediate_key_pem: String,
    root_pem: String,
}

pub(crate) fn generate() -> Result<Generated, Box<dyn std::error::Error + Send + Sync>> {
    let root_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut root_params = CertificateParams::new(vec!["RivetHub Test Root".to_owned()])?;
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    root_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    root_params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    root_params.not_after = rcgen::date_time_ymd(2036, 1, 1);
    let root = root_params.self_signed(&root_key)?;
    let intermediate_key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut intermediate_params =
        CertificateParams::new(vec!["RivetHub Test Intermediate".to_owned()])?;
    intermediate_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    intermediate_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    intermediate_params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    intermediate_params.not_after = rcgen::date_time_ymd(2035, 1, 1);
    let intermediate = intermediate_params.signed_by(
        &intermediate_key,
        &CaIssuer::from_params(&root_params, &root_key),
    )?;
    let intermediate_pem = intermediate.pem();
    let intermediate_key_pem = intermediate_key.serialize_pem();
    let root_pem = root.pem();
    Ok(Generated {
        issuer: TestAuthority {
            inner: Mutex::new(Inner {
                next: 1,
                tokens: BTreeMap::new(),
                issued: BTreeMap::new(),
                revoked: Vec::new(),
                intermediate_pem: intermediate_pem.clone(),
                intermediate_key_pem: intermediate_key_pem.clone(),
                root_pem: root_pem.clone(),
            }),
        },
        intermediate: intermediate_pem,
        intermediate_key: intermediate_key_pem,
        root: root_pem,
    })
}

impl TestAuthority {
    pub(crate) fn crl_pem(&self) -> Result<String, CaError> {
        let inner = self.inner.lock();
        let issuer = ca_issuer(&inner.intermediate_pem, &inner.intermediate_key_pem)?;
        let revoked_certs = inner
            .revoked
            .iter()
            .map(|serial| rcgen::RevokedCertParams {
                serial_number: SerialNumber::from_slice(serial),
                revocation_time: rcgen::date_time_ymd(2026, 6, 1),
                reason_code: None,
                invalidity_date: None,
            })
            .collect();
        let crl = CertificateRevocationListParams {
            this_update: rcgen::date_time_ymd(2026, 1, 1),
            next_update: rcgen::date_time_ymd(2027, 1, 1),
            crl_number: 1.into(),
            issuing_distribution_point: None,
            revoked_certs,
            key_identifier_method: KeyIdMethod::Sha256,
        }
        .signed_by(&issuer)
        .map_err(unavailable)?;
        let pem = crl.pem().map_err(unavailable)?;
        drop(inner);
        Ok(pem)
    }

    fn chain(&self) -> Vec<String> {
        let inner = self.inner.lock();
        vec![inner.intermediate_pem.clone(), inner.root_pem.clone()]
    }
}

#[async_trait::async_trait]
impl CertificateAuthority for TestAuthority {
    async fn issue_token(&self, principal: Principal) -> Result<EnrollmentToken, CaError> {
        let uri = principal.spiffe_uri()?;
        let mut inner = self.inner.lock();
        let next = inner.next.checked_add(1).ok_or_else(|| {
            CaError::Unavailable(Box::new(std::io::Error::other("token counter overflow")))
        })?;
        let token = format!("t{:032x}", inner.next);
        inner.next = next;
        inner.tokens.insert(token.clone(), uri);
        drop(inner);
        Ok(EnrollmentToken(token))
    }

    async fn sign(&self, csr: CsrPem, authority: SignAuthority) -> Result<CertChainPem, CaError> {
        let expected = expected_uri(&self.inner, authority)?;
        let issued = sign_request(&self.inner, &csr.0)?;
        if issued.uri != expected {
            return Err(CaError::Refused(
                "certificate request SAN does not match the principal".to_owned(),
            ));
        }
        record(&self.inner, &expected, issued.serial);
        Ok(CertChainPem {
            leaf: issued.leaf,
            chain: self.chain(),
        })
    }

    async fn chain(&self) -> Result<Vec<String>, CaError> {
        Ok(self.chain())
    }

    async fn crl(&self) -> Result<CrlPem, CaError> {
        self.crl_pem().map(CrlPem)
    }

    async fn revoke(&self, principal: Principal) -> Result<(), CaError> {
        let uri = principal.spiffe_uri()?;
        let mut inner = self.inner.lock();
        let Some(serials) = inner.issued.remove(&uri) else {
            return Err(CaError::Refused(
                "no certificate is recorded for this principal".to_owned(),
            ));
        };
        if serials.is_empty() {
            return Err(CaError::Refused(
                "no certificate is recorded for this principal".to_owned(),
            ));
        }
        inner.revoked.extend(serials);
        drop(inner);
        Ok(())
    }
}

struct SignedLeaf {
    leaf: String,
    serial: Vec<u8>,
    uri: String,
}

fn expected_uri(inner: &Mutex<Inner>, authority: SignAuthority) -> Result<String, CaError> {
    match authority {
        SignAuthority::Token(EnrollmentToken(token)) => {
            let mut guard = inner.lock();
            match guard.tokens.remove(&token) {
                Some(uri) => Ok(uri),
                None => Err(CaError::Refused(
                    "enrollment token is not active".to_owned(),
                )),
            }
        }
        SignAuthority::Renewal(principal) => principal.spiffe_uri(),
    }
}

fn sign_request(inner: &Mutex<Inner>, pem: &str) -> Result<SignedLeaf, CaError> {
    let mut request = CertificateSigningRequestParams::from_pem(pem).map_err(|_| {
        CaError::InvalidCsr("certificate request is not a PEM PKCS#10 request".to_owned())
    })?;
    let uri = single_uri(&request)?;
    request.params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    request.params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    request.params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    request.params.not_after = rcgen::date_time_ymd(2027, 1, 1);
    let (intermediate, key) = {
        let guard = inner.lock();
        (
            guard.intermediate_pem.clone(),
            guard.intermediate_key_pem.clone(),
        )
    };
    let issuer = ca_issuer(&intermediate, &key)?;
    let certificate = request.signed_by(&issuer).map_err(unavailable)?;
    let leaf = certificate.pem();
    let serial = raw_serial(&leaf)?;
    Ok(SignedLeaf { leaf, serial, uri })
}

fn record(inner: &Mutex<Inner>, uri: &str, serial: Vec<u8>) {
    let mut guard = inner.lock();
    match guard.issued.get_mut(uri) {
        Some(serials) => serials.push(serial),
        None => {
            guard.issued.insert(uri.to_owned(), vec![serial]);
        }
    }
}

fn single_uri(request: &CertificateSigningRequestParams) -> Result<String, CaError> {
    let mut uris = Vec::new();
    for name in &request.params.subject_alt_names {
        if let SanType::URI(uri) = name {
            uris.push(uri.to_string());
        }
    }
    match uris.as_slice() {
        [uri] => Ok(uri.clone()),
        _ => Err(CaError::Refused(
            "certificate request must contain exactly one URI SAN".to_owned(),
        )),
    }
}

fn ca_issuer<'a>(certificate: &'a str, key: &str) -> Result<CaIssuer<'a, KeyPair>, CaError> {
    let key = KeyPair::from_pem(key).map_err(unavailable)?;
    CaIssuer::from_ca_cert_pem(certificate, key).map_err(unavailable)
}

fn raw_serial(pem: &str) -> Result<Vec<u8>, CaError> {
    let der = CertificateDer::from_pem_slice(pem.as_bytes()).map_err(unavailable)?;
    let (_, certificate) =
        x509_parser::certificate::X509Certificate::from_der(der.as_ref()).map_err(unavailable)?;
    Ok(certificate.raw_serial().to_vec())
}

fn unavailable(error: impl std::error::Error + Send + Sync + 'static) -> CaError {
    CaError::Unavailable(Box::new(error))
}
