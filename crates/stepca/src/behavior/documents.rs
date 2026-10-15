use serde::Serialize;

use crate::Authority;
use crate::Error;
use crate::InvalidResponse;
use crate::harness::Reply;
use crate::harness::Server;
use crate::harness::TestResult;

use super::connected;
use super::csr_pem;
use super::document;
use super::ensure;
use super::fresh_root;
use super::get_step;
use super::sign_once;
use super::sign_response;
use super::sign_step;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_success_documents_are_rejected() -> TestResult {
    let server = Server::start().await?;
    let issued = server.issue()?;
    let csr = csr_pem()?;
    let authority = connected(&server)?;
    let too_large = sign_once(
        &server,
        &authority,
        &csr,
        document(201, &vec![b'x'; 1024 * 1024 + 1]),
    )
    .await;
    expect_invalid(too_large, "too large", |error| {
        matches!(error, InvalidResponse::TooLarge)
    })?;
    let plain = sign_once(
        &server,
        &authority,
        &csr,
        Reply::Document {
            status: 201,
            content_type: "text/plain",
            body: b"{}".to_vec(),
        },
    )
    .await;
    expect_invalid(plain, "plain", |error| {
        matches!(error, InvalidResponse::NotJson)
    })?;
    let unexpected = sign_once(
        &server,
        &authority,
        &csr,
        document(204, &serde_json::to_vec(&sign_response(&issued))?),
    )
    .await;
    expect_invalid(unexpected, "204", |error| {
        matches!(error, InvalidResponse::UnexpectedStatus(204))
    })?;
    let mut short = sign_response(&issued);
    short.cert_chain.pop();
    let chain = sign_once(
        &server,
        &authority,
        &csr,
        document(201, &serde_json::to_vec(&short)?),
    )
    .await;
    expect_invalid(chain, "chain", |error| {
        matches!(error, InvalidResponse::Chain)
    })?;
    let version = RawSigned {
        crt: &issued.leaf,
        ca: &issued.intermediate,
        cert_chain: vec![&issued.leaf, &issued.intermediate],
        tls_options: RawOptions {
            cipher_suites: vec!["TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256"],
            min_version: 1.1,
            max_version: 1.3,
            renegotiation: false,
        },
    };
    let old_tls = sign_once(
        &server,
        &authority,
        &csr,
        document(201, &serde_json::to_vec(&version)?),
    )
    .await;
    expect_invalid(old_tls, "tls version", |error| {
        matches!(error, InvalidResponse::Json(_))
    })?;
    server.close().await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn error_documents_keep_the_status_and_message() -> TestResult {
    let server = Server::start().await?;
    let csr = csr_pem()?;
    let authority = connected(&server)?;
    expect_rejected(
        sign_failure(
            &server,
            &authority,
            &csr,
            400,
            400,
            "missing client certificate.",
        )
        .await?,
        400,
        "missing client certificate.",
    )?;
    expect_refused(
        sign_failure(&server, &authority, &csr, 401, 401, "spent token").await?,
        401,
        "spent token",
    )?;
    expect_refused(
        sign_failure(&server, &authority, &csr, 403, 403, "san refused").await?,
        403,
        "san refused",
    )?;
    match sign_failure(&server, &authority, &csr, 503, 503, "offline").await? {
        Err(Error::Unavailable {
            status: 503,
            message,
        }) if message == "offline" => {}
        other => return Err(format!("503: {other:?}").into()),
    }
    match sign_failure(&server, &authority, &csr, 400, 401, "crossed").await? {
        Err(Error::Invalid(InvalidResponse::StatusMismatch {
            http: 400,
            body: 401,
        })) => {}
        other => return Err(format!("mismatch: {other:?}").into()),
    }
    match sign_failure(&server, &authority, &csr, 400, 400, "").await? {
        Err(Error::Invalid(InvalidResponse::EmptyMessage)) => {}
        other => return Err(format!("empty: {other:?}").into()),
    }
    server.push(sign_step(
        &csr,
        "token-value",
        Reply::Document {
            status: 400,
            content_type: "text/plain",
            body: b"nope".to_vec(),
        },
    ));
    match authority.sign(&csr, "token-value").await {
        Err(Error::Invalid(InvalidResponse::NotJson)) => server.close().await,
        other => Err(format!("text error: {other:?}").into()),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn roots_and_crl_must_match_the_configured_material() -> TestResult {
    let server = Server::start().await?;
    let authority = connected(&server)?;
    server.push(get_step(
        "/roots",
        None,
        document(
            201,
            &serde_json::to_vec(&RootsDocument {
                crts: vec![server.tls_pem().to_owned()],
            })?,
        ),
    ));
    let roots = authority.roots().await?;
    ensure(roots == vec![server.tls_pem().to_owned()], "roots")?;
    server.push(get_step(
        "/roots",
        None,
        document(201, &serde_json::to_vec(&RootsDocument { crts: vec![] })?),
    ));
    match authority.roots().await {
        Err(Error::Invalid(InvalidResponse::Roots)) => {}
        other => return Err(format!("empty roots: {other:?}").into()),
    }
    server.push(get_step(
        "/roots",
        None,
        document(
            201,
            &serde_json::to_vec(&RootsDocument {
                crts: vec![fresh_root()?],
            })?,
        ),
    ));
    match authority.roots().await {
        Err(Error::Invalid(InvalidResponse::Roots)) => {}
        other => return Err(format!("foreign root: {other:?}").into()),
    }
    server.push(get_step(
        "/1.0/crl",
        Some("pem=true".to_owned()),
        Reply::Document {
            status: 200,
            content_type: "application/x-pem-file",
            body: b"not a crl".to_vec(),
        },
    ));
    match authority.crl().await {
        Err(Error::Invalid(InvalidResponse::Crl)) => {}
        other => return Err(format!("crl text: {other:?}").into()),
    }
    server.push(get_step(
        "/1.0/crl",
        Some("pem=true".to_owned()),
        Reply::Document {
            status: 200,
            content_type: "application/x-pem-file",
            body: vec![0xff, 0xfe],
        },
    ));
    match authority.crl().await {
        Err(Error::Invalid(InvalidResponse::Encoding)) => server.close().await,
        other => Err(format!("crl bytes: {other:?}").into()),
    }
}

async fn sign_failure(
    server: &Server,
    authority: &Authority,
    csr: &str,
    http: u16,
    body_status: u16,
    message: &str,
) -> TestResult<Result<crate::SignedCertificate, Error>> {
    let body = serde_json::to_vec(&FailureDocument {
        status: body_status,
        message,
    })?;
    Ok(sign_once(server, authority, csr, document(http, &body)).await)
}

fn expect_invalid(
    result: Result<crate::SignedCertificate, Error>,
    label: &str,
    pred: impl FnOnce(&InvalidResponse) -> bool,
) -> TestResult {
    match result {
        Err(Error::Invalid(error)) if pred(&error) => Ok(()),
        other => Err(format!("{label}: {other:?}").into()),
    }
}

fn expect_rejected(
    result: Result<crate::SignedCertificate, Error>,
    status: u16,
    message: &str,
) -> TestResult {
    match result {
        Err(Error::Rejected {
            status: got,
            message: text,
        }) if got == status && text == message => Ok(()),
        other => Err(format!("rejected {status}: {other:?}").into()),
    }
}

fn expect_refused(
    result: Result<crate::SignedCertificate, Error>,
    status: u16,
    message: &str,
) -> TestResult {
    match result {
        Err(Error::Refused {
            status: got,
            message: text,
        }) if got == status && text == message => Ok(()),
        other => Err(format!("refused {status}: {other:?}").into()),
    }
}

#[derive(Serialize)]
struct FailureDocument<'a> {
    status: u16,
    message: &'a str,
}

#[derive(Serialize)]
struct RootsDocument {
    crts: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RawOptions {
    cipher_suites: Vec<&'static str>,
    min_version: f64,
    max_version: f64,
    renegotiation: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RawSigned<'a> {
    crt: &'a str,
    ca: &'a str,
    cert_chain: Vec<&'a str>,
    tls_options: RawOptions,
}
