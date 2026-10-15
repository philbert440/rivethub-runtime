mod support;

use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

use kernel::ca::CaError;
use kernel::ca::CertificateAuthority;
use kernel::ca::CsrPem;
use kernel::ca::EnrollmentToken;
use kernel::ca::Principal;
use kernel::ca::SignAuthority;
use rcgen::CertificateParams;
use rcgen::KeyPair;
use rcgen::SanType;
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use serde::Deserialize;
use support::Refusal;
use tokio_util::sync::CancellationToken;
use x509_parser::prelude::FromDer;

#[derive(Debug, Deserialize)]
struct Fixture {
    cases: Vec<Case>,
    #[serde(rename = "crlPem")]
    crl_pem: String,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
    request: CaseRequest,
    response: CaseResponse,
}

#[derive(Debug, Deserialize)]
struct CaseRequest {
    json: Option<RequestJson>,
}

#[derive(Debug, Deserialize)]
struct RequestJson {
    ott: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CaseResponse {
    status: u16,
    json: ResponseJson,
}

#[derive(Debug, Deserialize)]
struct ResponseJson {
    message: Option<String>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recorded_refusals_and_stepca_shape_pass_through_the_port()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(testkit::recording(
        "stepca/api.json",
    ))?)?;
    let reused = refusal(&fixture, "sign-token-reused")?;
    let forbidden = refusal(&fixture, "sign-san-not-granted")?;
    let stub = support::start(
        fixture.crl_pem.clone(),
        vec![reused.clone(), forbidden.clone()],
    )
    .await?;
    let authority = pki::Authority::connect(settings(&stub), CancellationToken::new()).await?;
    let roots = authority.chain().await?;
    assert_eq!(der(&roots)?, der(std::slice::from_ref(&stub.root_pem))?);
    let csr = csr_pem()?;
    let signed = authority
        .sign(
            CsrPem(csr),
            SignAuthority::Token(EnrollmentToken("fresh-token".to_owned())),
        )
        .await?;
    assert!(stub.saw("fresh-token"));
    assert!(signed.chain.len() >= 2);
    assert_eq!(signed.chain, authority.chain().await?);
    let tail = signed
        .chain
        .get(1..)
        .ok_or_else(|| missing("issued chain has no issuer"))?;
    assert_eq!(der(tail)?, der(std::slice::from_ref(&stub.root_pem))?);
    let reused_error = authority
        .sign(
            CsrPem(csr_pem()?),
            SignAuthority::Token(EnrollmentToken(reused.token.clone())),
        )
        .await
        .expect_err("spent token");
    assert_eq!(refused(reused_error)?, reused.message);
    let forbidden_error = authority
        .sign(
            CsrPem(csr_pem()?),
            SignAuthority::Token(EnrollmentToken(forbidden.token)),
        )
        .await
        .expect_err("ungranted san");
    assert_eq!(refused(forbidden_error)?, forbidden.message);
    let crl = authority.crl().await?;
    assert_eq!(crl.0, fixture.crl_pem);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unreachable_stepca_is_unavailable()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("root.pem");
    let password = directory.path().join("password");
    std::fs::write(&root, root_pem()?)?;
    std::fs::write(&password, "secret\n")?;
    let authority = pki::Authority::connect(
        pki::Settings {
            url: url::Url::parse(&format!("https://127.0.0.1:{port}/"))?,
            root,
            password_file: password,
            roster: None,
            environment: pki::ProcessEnv::from_pairs(Vec::<(&str, &str)>::new()),
        },
        CancellationToken::new(),
    )
    .await?;
    let error = tokio::time::timeout(Duration::from_secs(5), authority.chain())
        .await?
        .expect_err("closed port");
    assert!(matches!(error, CaError::Unavailable(_)), "{error}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoke_by_principal_calls_step_with_the_decimal_serial()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(testkit::recording(
        "stepca/api.json",
    ))?)?;
    let directory = tempfile::tempdir()?;
    let bin = directory.path().join("bin");
    std::fs::create_dir(&bin)?;
    let argv = directory.path().join("argv.txt");
    install_step(&bin)?;
    let stub = support::start(fixture.crl_pem, Vec::new()).await?;
    let mut settings = settings(&stub);
    settings.environment = pki::ProcessEnv::from_pairs([
        ("PATH", bin.to_str().ok_or("path")?),
        ("RIVETHUB_TEST_ARGV", argv.to_str().ok_or("argv")?),
    ]);
    let authority = pki::Authority::connect(settings, CancellationToken::new()).await?;
    let signed = authority
        .sign(
            CsrPem(csr_pem()?),
            SignAuthority::Token(EnrollmentToken("revoke-token".to_owned())),
        )
        .await?;
    authority
        .revoke(Principal::Node {
            node: "rec-node".to_owned(),
        })
        .await?;
    let recorded = std::fs::read_to_string(&argv)?;
    let serial = decimal_serial(&signed.leaf)?;
    assert!(recorded.lines().any(|line| line == "revoke"), "{recorded}");
    assert!(recorded.lines().any(|line| line == serial), "{recorded}");
    assert!(
        recorded.lines().all(|line| line != "--provisioner"),
        "{recorded}"
    );
    Ok(())
}

fn settings(stub: &support::Stub) -> pki::Settings {
    pki::Settings {
        url: stub.url.clone(),
        root: stub.root_path.clone(),
        password_file: stub.password_path.clone(),
        roster: None,
        environment: pki::ProcessEnv::from_pairs(Vec::<(&str, &str)>::new()),
    }
}

fn refusal(
    fixture: &Fixture,
    id: &str,
) -> Result<Refusal, Box<dyn std::error::Error + Send + Sync>> {
    let case = fixture
        .cases
        .iter()
        .find(|case| case.id == id)
        .ok_or_else(|| missing(id))?;
    let token = case
        .request
        .json
        .as_ref()
        .and_then(|json| json.ott.clone())
        .ok_or_else(|| missing("recorded token"))?;
    let message = case
        .response
        .json
        .message
        .clone()
        .ok_or_else(|| missing("recorded message"))?;
    Ok(Refusal {
        token,
        status: case.response.status,
        message,
    })
}

fn missing(detail: &str) -> Box<dyn std::error::Error + Send + Sync> {
    std::io::Error::other(detail.to_owned()).into()
}

fn refused(error: CaError) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    match error {
        CaError::Refused(message) => Ok(message),
        other @ (CaError::InvalidCsr(_) | CaError::Unavailable(_)) => Err(other.to_string().into()),
    }
}

fn csr_pem() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut params = CertificateParams::default();
    params.subject_alt_names = vec![SanType::URI(
        "spiffe://rivet.mesh/node/rec-node".try_into()?,
    )];
    Ok(params.serialize_request(&key)?.pem()?)
}

fn der(pems: &[String]) -> Result<Vec<Vec<u8>>, Box<dyn std::error::Error + Send + Sync>> {
    let mut certificates = Vec::new();
    for pem in pems {
        certificates.push(
            CertificateDer::from_pem_slice(pem.as_bytes())?
                .as_ref()
                .to_vec(),
        );
    }
    Ok(certificates)
}

fn decimal_serial(pem: &str) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let der = CertificateDer::from_pem_slice(pem.as_bytes())?;
    let (_, certificate) = x509_parser::prelude::X509Certificate::from_der(der.as_ref())?;
    let serial = num_bigint::BigUint::from_bytes_be(certificate.raw_serial()).to_str_radix(10);
    Ok(serial)
}

fn root_pem() -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
    let mut params = CertificateParams::new(vec!["RivetHub Test Root".to_owned()])?;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    params.not_after = rcgen::date_time_ymd(2036, 1, 1);
    Ok(params.self_signed(&key)?.pem())
}

fn install_step(bin: &std::path::Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let step = bin.join("step");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o755)
        .open(&step)?;
    std::io::Write::write_all(
        &mut file,
        br#"#!/bin/sh
{
  printf '%s\n' "$0"
  for arg do
    printf '%s\n' "$arg"
  done
} > "$RIVETHUB_TEST_ARGV"
printf '%s\n' 'one-time-token'
"#,
    )?;
    Ok(())
}
