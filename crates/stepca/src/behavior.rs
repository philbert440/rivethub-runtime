use std::time::Duration;

use rcgen::BasicConstraints;
use rcgen::CertificateParams;
use rcgen::IsCa;
use rcgen::KeyPair;
use rcgen::KeyUsagePurpose;
use tokio_util::sync::CancellationToken;

use crate::Authority;
use crate::CipherSuite;
use crate::Config;
use crate::Error;
use crate::Holder;
use crate::Serial;
use crate::TlsOptions;
use crate::TlsVersion;
use crate::document::SignResponse;
use crate::harness::ExpectedBody;
use crate::harness::Issued;
use crate::harness::Peer;
use crate::harness::Reply;
use crate::harness::Server;
use crate::harness::Step;
use crate::harness::TestResult;

mod documents;

#[test]
fn address_must_be_an_https_origin() -> TestResult {
    let root = fresh_root()?;
    let rejected = [
        ("http://127.0.0.1/", ErrorKind::Scheme),
        ("https://user:secret@127.0.0.1/", ErrorKind::Credentials),
        ("https://127.0.0.1/?q=1", ErrorKind::Query),
        ("https://127.0.0.1/#fragment", ErrorKind::Fragment),
        ("https://127.0.0.1/extra", ErrorKind::Path),
    ];
    for (raw, expected) in rejected {
        let error = Config::new(raw.parse()?, "unused".to_owned());
        match error {
            Err(Error::Address(fault)) if fault_kind(fault) == expected => {}
            other => return Err(format!("{raw}: {other:?}").into()),
        }
    }
    Config::new("https://stepca.example.test".parse()?, root.clone())?;
    Config::new("https://stepca.example.test/".parse()?, root)?;
    match Config::new("https://stepca.example.test/".parse()?, "nope".to_owned()) {
        Err(Error::Root) => {}
        other => return Err(format!("bad root: {other:?}").into()),
    }
    let doubled = format!("{}{}", fresh_root()?, fresh_root()?);
    match Config::new("https://stepca.example.test/".parse()?, doubled) {
        Err(Error::Root) => Ok(()),
        other => Err(format!("two roots: {other:?}").into()),
    }
}

#[tokio::test]
async fn requests_are_rejected_before_they_are_sent() -> TestResult {
    let authority = Authority::connect(Config::new(
        "https://stepca.example.test/".parse()?,
        fresh_root()?,
    )?)?;
    let csr = csr_pem()?;
    match authority.sign("not a request", "token-value").await {
        Err(Error::Csr) => {}
        other => return Err(format!("csr: {other:?}").into()),
    }
    match authority.sign(&fresh_root()?, "token-value").await {
        Err(Error::Csr) => {}
        other => return Err(format!("certificate used as csr: {other:?}").into()),
    }
    for token in ["", " ", "token value", "token\nvalue", "\ttoken"] {
        match authority.sign(&csr, token).await {
            Err(Error::Token) => {}
            other => return Err(format!("token {token:?}: {other:?}").into()),
        }
    }
    let serial = "153723891141980687060330047892021838391";
    ensure(Serial::parse(serial)?.as_str() == serial, "serial text")?;
    ensure(Serial::parse("0").is_ok(), "zero")?;
    ensure(Serial::parse("10").is_ok(), "ten")?;
    ensure(Serial::parse(&"9".repeat(128)).is_ok(), "128 digits")?;
    for rejected in ["", "00", "01", "12a", " 1", &"9".repeat(129)] {
        ensure(Serial::parse(rejected).is_err(), rejected)?;
    }
    Ok(())
}

#[test]
fn holder_debug_hides_the_private_key() -> TestResult {
    let key = KeyPair::generate()?;
    let params = CertificateParams::new(vec!["holder.example.test".to_owned()])?;
    let certificate = params.self_signed(&key)?;
    let holder = Holder::from_pem(&certificate.pem(), &key.serialize_pem())?;
    let rendered = format!("{holder:?}");
    ensure(!rendered.contains("PRIVATE"), &rendered)?;
    match Holder::from_pem("not a certificate", &key.serialize_pem()) {
        Err(Error::HolderCertificate) => {}
        other => return Err(format!("certificate: {other:?}").into()),
    }
    match Holder::from_pem(&certificate.pem(), "not a key") {
        Err(Error::Holder(_)) => Ok(()),
        other => Err(format!("key: {other:?}").into()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_signs_against_the_configured_root() -> TestResult {
    let server = Server::start().await?;
    let issued = server.issue()?;
    let csr = csr_pem()?;
    let authority = connected(&server)?;
    let signed = sign_issued(&server, &authority, &csr, &issued).await?;
    ensure(signed.leaf == issued.leaf, "leaf")?;
    ensure(signed.intermediate == issued.intermediate, "intermediate")?;
    ensure(
        signed.chain == vec![issued.leaf.clone(), issued.intermediate.clone()],
        "chain",
    )?;
    ensure(signed.tls == tls_options(), "tls")?;
    server.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handshake_requires_the_configured_root() -> TestResult {
    let server = Server::start().await?;
    let issued = server.issue()?;
    let csr = csr_pem()?;
    let authority = Authority::connect(Config::new(server.url().clone(), fresh_root()?)?)?;
    server.push(sign_step(
        &csr,
        "token-value",
        document(201, &serde_json::to_vec(&sign_response(&issued))?),
    ));
    match authority.sign(&csr, "token-value").await {
        Err(Error::Transport(_)) => server.stop().await,
        other => Err(format!("expected a transport failure: {other:?}").into()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn issued_chain_must_match_the_configured_root() -> TestResult {
    let server = Server::start().await?;
    let issued = server.issue()?;
    let csr = csr_pem()?;
    let authority = Authority::for_replay(server.url().clone(), &fresh_root()?, server.tls_pem())?;
    server.push(sign_step(
        &csr,
        "token-value",
        document(201, &serde_json::to_vec(&sign_response(&issued))?),
    ));
    match authority.sign(&csr, "token-value").await {
        Err(Error::Untrusted(_)) => server.close().await,
        other => Err(format!("expected an untrusted chain: {other:?}").into()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn same_origin_redirect_preserves_the_post() -> TestResult {
    let server = Server::start().await?;
    let issued = server.issue()?;
    let csr = csr_pem()?;
    let location = server.url().join("1.0/sign")?.to_string();
    server.push(sign_step(
        &csr,
        "token-value",
        Reply::Redirect {
            status: 307,
            location,
        },
    ));
    let signed = sign_issued(&server, &connected(&server)?, &csr, &issued).await?;
    ensure(signed.leaf == issued.leaf, "leaf after redirect")?;
    server.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redirects_that_drop_the_body_or_leave_the_origin_are_refused() -> TestResult {
    let server = Server::start().await?;
    let csr = csr_pem()?;
    let authority = connected(&server)?;
    let location = server.url().join("1.0/sign")?.to_string();
    server.push(sign_step(
        &csr,
        "token-value",
        Reply::Redirect {
            status: 302,
            location,
        },
    ));
    match authority.sign(&csr, "token-value").await {
        Err(Error::Transport(_)) => {}
        other => return Err(format!("302: {other:?}").into()),
    }
    server.push(sign_step(
        &csr,
        "token-value",
        Reply::Redirect {
            status: 307,
            location: "https://example.test/1.0/sign".to_owned(),
        },
    ));
    match authority.sign(&csr, "token-value").await {
        Err(Error::Transport(_)) => server.close().await,
        other => Err(format!("cross-origin: {other:?}").into()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn too_many_redirects_are_refused() -> TestResult {
    let server = Server::start().await?;
    let csr = csr_pem()?;
    let location = server.url().join("1.0/sign")?.to_string();
    for _ in 0..5 {
        server.push(sign_step(
            &csr,
            "token-value",
            Reply::Redirect {
                status: 307,
                location: location.clone(),
            },
        ));
    }
    match connected(&server)?.sign(&csr, "token-value").await {
        Err(Error::Transport(_)) => server.close().await,
        other => Err(format!("redirect limit: {other:?}").into()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_stops_a_held_sign() -> TestResult {
    let server = Server::start().await?;
    let csr = csr_pem()?;
    server.push(sign_step(&csr, "token-value", Reply::Hold));
    let cancel = CancellationToken::new();
    let authority = connected(&server)?.with_cancellation(cancel.clone());
    let pem = csr.clone();
    let task = tokio::spawn(async move { authority.sign(&pem, "token-value").await });
    tokio::time::timeout(Duration::from_secs(5), server.until_accepted()).await?;
    cancel.cancel();
    match task.await? {
        Err(Error::Cancelled) => server.close().await,
        other => Err(format!("expected cancellation: {other:?}").into()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ErrorKind {
    Scheme,
    Host,
    Credentials,
    Query,
    Fragment,
    Path,
}

fn fault_kind(fault: crate::AddressFault) -> ErrorKind {
    match fault {
        crate::AddressFault::Scheme => ErrorKind::Scheme,
        crate::AddressFault::Host => ErrorKind::Host,
        crate::AddressFault::Credentials => ErrorKind::Credentials,
        crate::AddressFault::Query => ErrorKind::Query,
        crate::AddressFault::Fragment => ErrorKind::Fragment,
        crate::AddressFault::Path => ErrorKind::Path,
    }
}

pub(crate) fn ensure(condition: bool, detail: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(detail.into())
    }
}

pub(crate) fn fresh_root() -> TestResult<String> {
    let key = KeyPair::generate()?;
    let mut params = CertificateParams::new(vec!["other.example.test".to_owned()])?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    Ok(params.self_signed(&key)?.pem())
}

pub(crate) fn csr_pem() -> TestResult<String> {
    let key = KeyPair::generate()?;
    let params = CertificateParams::new(vec!["node.example.test".to_owned()])?;
    Ok(params.serialize_request(&key)?.pem()?)
}

fn tls_options() -> TlsOptions {
    TlsOptions {
        cipher_suites: vec![CipherSuite::Chacha20Poly1305, CipherSuite::Aes128Gcm],
        min_version: TlsVersion::V12,
        max_version: TlsVersion::V13,
        renegotiation: false,
    }
}

pub(crate) fn sign_response(issued: &Issued) -> SignResponse {
    SignResponse {
        crt: issued.leaf.clone(),
        ca: issued.intermediate.clone(),
        cert_chain: vec![issued.leaf.clone(), issued.intermediate.clone()],
        tls_options: tls_options(),
    }
}

pub(crate) fn connected(server: &Server) -> TestResult<Authority> {
    Ok(Authority::connect(Config::new(
        server.url().clone(),
        server.tls_pem().to_owned(),
    )?)?)
}

pub(crate) fn document(status: u16, body: &[u8]) -> Reply {
    Reply::Document {
        status,
        content_type: "application/json",
        body: body.to_vec(),
    }
}

pub(crate) fn sign_step(csr: &str, token: &str, reply: Reply) -> Step {
    Step {
        method: "POST".to_owned(),
        path: "/1.0/sign".to_owned(),
        query: None,
        peer: Peer::Absent,
        body: ExpectedBody::Sign {
            csr: csr.to_owned(),
            ott: token.to_owned(),
        },
        reply,
    }
}

pub(crate) fn get_step(path: &str, query: Option<String>, reply: Reply) -> Step {
    Step {
        method: "GET".to_owned(),
        path: path.to_owned(),
        query,
        peer: Peer::Absent,
        body: ExpectedBody::Empty,
        reply,
    }
}

async fn sign_issued(
    server: &Server,
    authority: &Authority,
    csr: &str,
    issued: &Issued,
) -> TestResult<crate::SignedCertificate> {
    let body = serde_json::to_vec(&sign_response(issued))?;
    Ok(sign_once(server, authority, csr, document(201, &body)).await?)
}

pub(crate) async fn sign_once(
    server: &Server,
    authority: &Authority,
    csr: &str,
    reply: Reply,
) -> Result<crate::SignedCertificate, Error> {
    server.push(sign_step(csr, "token-value", reply));
    authority.sign(csr, "token-value").await
}
