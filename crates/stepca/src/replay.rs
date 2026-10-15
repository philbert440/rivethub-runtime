use serde::Deserialize;
use serde::Serialize;

use crate::Authority;
use crate::Error;
use crate::Holder;
use crate::Revocation;
use crate::RevocationCause;
use crate::Serial;
use crate::SignedCertificate;
use crate::document::SignResponse;
use crate::harness::ExpectedBody;
use crate::harness::Minted;
use crate::harness::Peer;
use crate::harness::Reply;
use crate::harness::Server;
use crate::harness::Step;
use crate::harness::TestResult;

const REPLAYED: &[&str] = &[
    "sign-with-token",
    "sign-token-reused",
    "sign-san-not-granted",
    "renew-mtls",
    "renew-without-certificate",
    "revoke-own-mtls",
    "renew-revoked",
    "roots",
];

const SKIPPED: &[(&str, &str)] = &[];

#[test]
fn every_recorded_exchange_is_replayed_or_skipped() -> TestResult {
    let fixture = load()?;
    ensure(!fixture.recorded.is_empty(), "recording label")?;
    ensure(!fixture.crl_pem.is_empty(), "crl")?;
    ensure(
        fixture.cases.len() == REPLAYED.len() + SKIPPED.len(),
        "case count",
    )?;
    let mut seen = Vec::new();
    for case in &fixture.cases {
        ensure(!case.description.is_empty(), "description")?;
        let replayed = REPLAYED.contains(&case.id.as_str());
        let skipped = SKIPPED
            .iter()
            .any(|(name, reason)| *name == case.id && reason.len() >= 60);
        ensure(replayed ^ skipped, &case.id)?;
        ensure(!seen.contains(&case.id.as_str()), "duplicate case")?;
        seen.push(case.id.as_str());
    }
    let mut prior = Vec::new();
    for name in REPLAYED {
        ensure(!prior.contains(name), "duplicate replay name")?;
        ensure(seen.contains(name), name)?;
        prior.push(*name);
    }
    for (name, reason) in SKIPPED {
        ensure(reason.len() >= 60, name)?;
        ensure(seen.contains(name), name)?;
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recorded_step_ca_exchanges_replay() -> TestResult {
    let fixture = load()?;
    let root = root_pem(&fixture)?.to_owned();
    let server = Server::start().await?;
    let current = server.mint("current")?;
    let revoked = server.mint("revoked")?;
    let authority = Authority::for_replay(server.url().clone(), &root, server.tls_pem())?;
    for case in &fixture.cases {
        if let Err(error) = replay_case(&server, &authority, &current, &revoked, case).await {
            return Err(format!("{}: {error}", case.id).into());
        }
    }
    replay_crl(&server, &authority, &fixture.crl_pem).await?;
    server.close().await
}

async fn replay_case(
    server: &Server,
    authority: &Authority,
    current: &Minted,
    revoked: &Minted,
    case: &Case,
) -> TestResult {
    server.push(step_for(case, current, revoked)?);
    let holder = holder_for(case, current, revoked);
    let observed = perform(authority, case, holder).await?;
    if let Some(mismatch) = server.mismatch() {
        return Err(mismatch.into());
    }
    judge(case, observed)
}

async fn replay_crl(server: &Server, authority: &Authority, pem: &str) -> TestResult {
    server.push(Step {
        method: "GET".to_owned(),
        path: "/1.0/crl".to_owned(),
        query: Some("pem=true".to_owned()),
        peer: Peer::Absent,
        body: ExpectedBody::Empty,
        reply: Reply::Document {
            status: 200,
            content_type: "application/x-pem-file",
            body: pem.as_bytes().to_vec(),
        },
    });
    let crl = authority.crl().await?;
    ensure(crl == pem, "crl text")?;
    if let Some(mismatch) = server.mismatch() {
        return Err(mismatch.into());
    }
    Ok(())
}

async fn perform(
    authority: &Authority,
    case: &Case,
    holder: Option<&Holder>,
) -> TestResult<Observed> {
    match (
        case.request.method.as_str(),
        case.request.path.as_str(),
        holder,
    ) {
        ("POST", "/1.0/sign", None) => Ok(signed(
            authority.sign(sign_csr(case)?, sign_token(case)?).await,
        )),
        ("POST", "/1.0/renew", Some(holder)) => Ok(signed(authority.renew(holder).await)),
        ("POST", "/1.0/renew", None) => Ok(signed(authority.renew_without_holder().await)),
        ("POST", "/1.0/revoke", Some(holder)) => {
            Ok(revoked(authority.revoke(holder, &revocation(case)?).await))
        }
        ("GET", "/roots", None) => match authority.roots().await {
            Ok(roots) => Ok(Observed::Roots(roots)),
            Err(error) => Ok(Observed::Failed(error)),
        },
        (method, path, _) => Err(format!("no client call for {method} {path}").into()),
    }
}

fn signed(result: Result<SignedCertificate, Error>) -> Observed {
    match result {
        Ok(signed) => Observed::Signed(signed),
        Err(error) => Observed::Failed(error),
    }
}

fn revoked(result: Result<(), Error>) -> Observed {
    match result {
        Ok(()) => Observed::Ack,
        Err(error) => Observed::Failed(error),
    }
}

fn judge(case: &Case, observed: Observed) -> TestResult {
    match observed {
        Observed::Signed(signed) => expect_signed(case, &signed),
        Observed::Roots(roots) => expect_roots(case, &roots),
        Observed::Ack => expect_ack(case),
        Observed::Failed(error) => expect_failure(case, &error),
    }
}

fn expect_signed(case: &Case, signed: &SignedCertificate) -> TestResult {
    let ResponseDocument::Signed(response) = &case.response.json else {
        return Err("expected a signed certificate".into());
    };
    ensure(case.response.status == 201, "sign status")?;
    ensure(signed.leaf == response.crt, "leaf")?;
    ensure(signed.intermediate == response.ca, "intermediate")?;
    ensure(signed.chain == response.cert_chain, "chain")?;
    ensure(signed.tls == response.tls_options, "tls")?;
    Ok(())
}

fn expect_roots(case: &Case, roots: &[String]) -> TestResult {
    let ResponseDocument::Roots(response) = &case.response.json else {
        return Err("expected roots".into());
    };
    ensure(case.response.status == 201, "roots status")?;
    ensure(roots == response.crts, "roots")?;
    Ok(())
}

fn expect_ack(case: &Case) -> TestResult {
    let ResponseDocument::Ack(response) = &case.response.json else {
        return Err("expected an acknowledgement".into());
    };
    ensure(case.response.status == 200, "revoke status")?;
    ensure(response.status == AckWord::Ok, "ack")?;
    Ok(())
}

fn expect_failure(case: &Case, error: &Error) -> TestResult {
    let ResponseDocument::Failure(body) = &case.response.json else {
        return Err(format!("expected a failure document: {error}").into());
    };
    ensure(body.status == case.response.status, "document status")?;
    let matched = matches!(
        (error, case.response.status),
        (Error::Rejected { status, message }, 400)
            | (Error::Refused { status, message }, 401 | 403)
            if *status == case.response.status && message == &body.message
    );
    if matched {
        Ok(())
    } else {
        Err(format!("{}: {error}", case.id).into())
    }
}

fn step_for(case: &Case, current: &Minted, revoked: &Minted) -> TestResult<Step> {
    ensure(
        case.response.content_type == "application/json",
        "content type",
    )?;
    Ok(Step {
        method: case.request.method.clone(),
        path: case.request.path.clone(),
        query: None,
        peer: peer(case, current, revoked),
        body: expected_body(case.request.json.as_ref()),
        reply: Reply::Document {
            status: case.response.status,
            content_type: "application/json",
            body: serde_json::to_vec(&case.response.json)?,
        },
    })
}

fn peer(case: &Case, current: &Minted, revoked: &Minted) -> Peer {
    match case.request.client_certificate {
        ClientCertificate::None => Peer::Absent,
        ClientCertificate::Current => Peer::Der(current.der.clone()),
        ClientCertificate::Revoked => Peer::Der(revoked.der.clone()),
    }
}

fn holder_for<'a>(case: &Case, current: &'a Minted, revoked: &'a Minted) -> Option<&'a Holder> {
    match case.request.client_certificate {
        ClientCertificate::None => None,
        ClientCertificate::Current => Some(&current.holder),
        ClientCertificate::Revoked => Some(&revoked.holder),
    }
}

fn expected_body(document: Option<&RequestObject>) -> ExpectedBody {
    match document {
        None => ExpectedBody::Empty,
        Some(RequestObject::Sign(sign)) => ExpectedBody::Sign {
            csr: sign.csr.clone(),
            ott: sign.ott.clone(),
        },
        Some(RequestObject::Revoke(revoke)) => ExpectedBody::Revoke {
            serial: revoke.serial.clone(),
            reason_code: revoke.reason_code,
            reason: revoke.reason,
            passive: revoke.passive,
        },
    }
}

fn sign_csr(case: &Case) -> TestResult<&str> {
    match &case.request.json {
        Some(RequestObject::Sign(sign)) => Ok(sign.csr.as_str()),
        _ => Err("sign request".into()),
    }
}

fn sign_token(case: &Case) -> TestResult<&str> {
    match &case.request.json {
        Some(RequestObject::Sign(sign)) => Ok(sign.ott.as_str()),
        _ => Err("sign token".into()),
    }
}

fn revocation(case: &Case) -> TestResult<Revocation> {
    let Some(RequestObject::Revoke(revoke)) = &case.request.json else {
        return Err("revoke request".into());
    };
    ensure(revoke.reason.code() == revoke.reason_code, "reason code")?;
    Ok(Revocation {
        serial: Serial::parse(&revoke.serial)?,
        reason: revoke.reason,
        passive: revoke.passive,
    })
}

fn root_pem(fixture: &Fixture) -> TestResult<&str> {
    let case = fixture
        .cases
        .iter()
        .find(|case| case.id == "roots")
        .ok_or("roots case")?;
    let ResponseDocument::Roots(response) = &case.response.json else {
        return Err("roots document".into());
    };
    match response.crts.as_slice() {
        [pem] => Ok(pem.as_str()),
        _ => Err("one configured root".into()),
    }
}

fn load() -> TestResult<Fixture> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testkit/tests/fixtures/stepca/api.json");
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn ensure(condition: bool, detail: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(detail.into())
    }
}

enum Observed {
    Signed(SignedCertificate),
    Roots(Vec<String>),
    Ack,
    Failed(Error),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    recorded: String,
    cases: Vec<Case>,
    #[serde(rename = "crlPem")]
    crl_pem: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    description: String,
    request: CaseRequest,
    response: CaseResponse,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CaseRequest {
    method: String,
    path: String,
    client_certificate: ClientCertificate,
    json: Option<RequestObject>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct CaseResponse {
    status: u16,
    content_type: String,
    json: ResponseDocument,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum RequestObject {
    Sign(SignRequest),
    Revoke(RevokeRequest),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignRequest {
    csr: String,
    ott: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RevokeRequest {
    serial: String,
    reason_code: u8,
    reason: RevocationCause,
    passive: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum ResponseDocument {
    Signed(SignResponse),
    Failure(FailureBody),
    Ack(AckBody),
    Roots(RootsBody),
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct FailureBody {
    status: u16,
    message: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct AckBody {
    status: AckWord,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
enum AckWord {
    #[serde(rename = "ok")]
    Ok,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RootsBody {
    crts: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
enum ClientCertificate {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "current")]
    Current,
    #[serde(rename = "revoked")]
    Revoked,
}
