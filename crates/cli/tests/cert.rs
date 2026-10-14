use rustls_pki_types::pem::PemObject;
#[path = "pki_support/server.rs"]
mod server;
#[path = "pki_support/mod.rs"]
mod support;

use cli::Exit;
use kernel::ca::CertificateAuthority;
use server::{Behavior, Server};
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use support::{Result, directory, run, text};
use tokio_util::sync::CancellationToken;
use x509_parser::prelude::FromDer;

#[tokio::test]
async fn enrolls_with_token_then_renews_using_verified_mtls_and_keeps_key_local() -> Result {
    let home = directory()?;
    let state = home.path().join("ca");
    let authority = pki::Pki::initialize(
        state.clone(),
        home.path().join("offline.pem"),
        CancellationToken::new(),
    )
    .await?;
    let token = authority
        .issue_token("spiffe://rivet.mesh/node/node-a".parse()?)
        .await?;
    let token_file = home.path().join("token");
    std::fs::write(&token_file, &token.0)?;
    let mut server = Server::start(authority, &state, Behavior::Redirect).await?;
    let root = state.join("root.cert.pem");
    let args = [
        "cert",
        "renew",
        "--principal",
        "spiffe://rivet.mesh/node/node-a",
        "--ca-url",
        server.url.as_str(),
        "--trust-root",
        text(&root)?,
        "--once",
    ];
    let mut enroll = args.to_vec();
    enroll.extend(["--token-file", text(&token_file)?]);
    let report = run(home.path(), &enroll).await?;
    assert_eq!(report.exit, Exit::Ok, "{}", report.stderr);
    assert_eq!(report.stdout, "");
    assert_eq!(report.stderr, "");
    assert!(
        !tokio::time::timeout(Duration::from_secs(5), server.events.recv())
            .await?
            .ok_or("missing enrollment")?
    );
    let holder = home.path().join(".rivethub/cert");
    let key = std::fs::read(holder.join("holder.key.pem"))?;
    assert_eq!(
        std::fs::metadata(holder.join("holder.key.pem"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let before = std::fs::read(holder.join("holder.cert.pem"))?;
    let report = run(home.path(), &args).await?;
    assert_eq!(report.exit, Exit::Ok, "{}", report.stderr);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), server.events.recv())
            .await?
            .ok_or("missing renewal")?
    );
    assert_eq!(key, std::fs::read(holder.join("holder.key.pem"))?);
    assert_ne!(before, std::fs::read(holder.join("holder.cert.pem"))?);
    assert!(
        !String::from_utf8(std::fs::read(holder.join("holder.cert.pem"))?)?.contains("PRIVATE KEY")
    );
    let duplicate = run(home.path(), &enroll).await?;
    assert_eq!(duplicate.exit, Exit::Failure);
    assert!(!duplicate.stderr.contains(&token.0));
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn rejects_wrong_keys_identities_cross_origin_redirects_and_refusals_without_publishing()
-> Result {
    for behavior in [
        Behavior::WrongKey,
        Behavior::WrongPrincipal,
        Behavior::ForeignRedirect,
        Behavior::Refuse,
    ] {
        let home = directory()?;
        let state = home.path().join("ca");
        let authority = pki::Pki::initialize(
            state.clone(),
            home.path().join("offline.pem"),
            CancellationToken::new(),
        )
        .await?;
        let token = authority
            .issue_token("spiffe://rivet.mesh/node/node-a".parse()?)
            .await?;
        let token_file = home.path().join("token");
        std::fs::write(&token_file, &token.0)?;
        let server = Server::start(authority, &state, behavior).await?;
        let report = run(
            home.path(),
            &[
                "cert",
                "renew",
                "--principal",
                "spiffe://rivet.mesh/node/node-a",
                "--ca-url",
                server.url.as_str(),
                "--trust-root",
                text(&state.join("root.cert.pem"))?,
                "--token-file",
                text(&token_file)?,
                "--once",
            ],
        )
        .await?;
        assert_eq!(report.exit, Exit::Failure, "{behavior:?}");
        assert_eq!(report.stdout, "");
        assert!(!report.stderr.contains(&token.0));
        assert!(!home.path().join(".rivethub/cert/holder.cert.pem").exists());
        server.stop().await?;
    }
    Ok(())
}

#[tokio::test]
async fn long_running_renewal_obeys_cancellation_and_does_not_renew_early() -> Result {
    let home = directory()?;
    let state = home.path().join("ca");
    let authority = pki::Pki::initialize(
        state.clone(),
        home.path().join("offline.pem"),
        CancellationToken::new(),
    )
    .await?;
    let token = authority
        .issue_token("spiffe://rivet.mesh/node/node-a".parse()?)
        .await?;
    let token_file = home.path().join("token");
    std::fs::write(&token_file, &token.0)?;
    let mut server = Server::start(authority, &state, Behavior::Sign).await?;
    let holder = home.path().join("holder");
    let args = cli::cert::RenewArgs {
        principal: "spiffe://rivet.mesh/node/node-a".parse()?,
        ca_url: server.url.clone(),
        trust_root: state.join("root.cert.pem"),
        token_file: Some(token_file),
        state: None,
        once: true,
    };
    cli::cert::renew(args, holder.clone(), &CancellationToken::new()).await?;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), server.events.recv()).await?,
        Some(false)
    );
    let certificate = std::fs::read_to_string(holder.join("holder.cert.pem"))?;
    let leaf = rustls_pki_types::CertificateDer::from_pem_slice(certificate.as_bytes())?;
    let (_, parsed) = x509_parser::certificate::X509Certificate::from_der(&leaf)?;
    assert_eq!(
        parsed.validity().not_after.timestamp() - parsed.validity().not_before.timestamp(),
        7 * 86400
    );
    let args = cli::cert::RenewArgs {
        principal: "spiffe://rivet.mesh/node/node-a".parse()?,
        ca_url: server.url.clone(),
        trust_root: state.join("root.cert.pem"),
        token_file: None,
        state: None,
        once: false,
    };
    let shutdown = CancellationToken::new();
    let token = shutdown.clone();
    let task = tokio::spawn(async move { cli::cert::renew(args, holder, &token).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), server.events.recv())
            .await
            .is_err()
    );
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), task).await???;
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn cancellation_interrupts_an_inflight_signing_request() -> Result {
    let home = directory()?;
    let state = home.path().join("ca");
    let authority = pki::Pki::initialize(
        state.clone(),
        home.path().join("offline.pem"),
        CancellationToken::new(),
    )
    .await?;
    let token = authority
        .issue_token("spiffe://rivet.mesh/node/node-a".parse()?)
        .await?;
    let token_file = home.path().join("token");
    std::fs::write(&token_file, &token.0)?;
    let mut server = Server::start(authority, &state, Behavior::Wait).await?;
    let holder = home.path().join("holder");
    let args = cli::cert::RenewArgs {
        principal: "spiffe://rivet.mesh/node/node-a".parse()?,
        ca_url: server.url.clone(),
        trust_root: state.join("root.cert.pem"),
        token_file: Some(token_file),
        state: None,
        once: false,
    };
    let shutdown = CancellationToken::new();
    let stop = shutdown.clone();
    let path = holder.clone();
    let task = tokio::spawn(async move { cli::cert::renew(args, path, &stop).await });
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), server.events.recv()).await?,
        Some(false)
    );
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(5), task).await???;
    assert!(!holder.join("holder.cert.pem").exists());
    server.stop().await?;
    Ok(())
}

#[tokio::test]
async fn rejected_renewal_preserves_the_existing_certificate_and_key() -> Result {
    let home = directory()?;
    let state = home.path().join("ca");
    let authority = pki::Pki::initialize(
        state.clone(),
        home.path().join("offline.pem"),
        CancellationToken::new(),
    )
    .await?;
    let token = authority
        .issue_token("spiffe://rivet.mesh/node/node-a".parse()?)
        .await?;
    let token_file = home.path().join("token");
    std::fs::write(&token_file, &token.0)?;
    let server = Server::start(authority.clone(), &state, Behavior::Sign).await?;
    let holder = home.path().join("holder");
    let args = cli::cert::RenewArgs {
        principal: "spiffe://rivet.mesh/node/node-a".parse()?,
        ca_url: server.url.clone(),
        trust_root: state.join("root.cert.pem"),
        token_file: Some(token_file),
        state: None,
        once: true,
    };
    cli::cert::renew(args, holder.clone(), &CancellationToken::new()).await?;
    server.stop().await?;
    let before = std::fs::read(holder.join("holder.cert.pem"))?;
    let key = std::fs::read(holder.join("holder.key.pem"))?;
    let server = Server::start(authority, &state, Behavior::WrongKey).await?;
    let args = cli::cert::RenewArgs {
        principal: "spiffe://rivet.mesh/node/node-a".parse()?,
        ca_url: server.url.clone(),
        trust_root: state.join("root.cert.pem"),
        token_file: None,
        state: None,
        once: true,
    };
    assert!(
        cli::cert::renew(args, holder.clone(), &CancellationToken::new())
            .await
            .is_err()
    );
    assert_eq!(before, std::fs::read(holder.join("holder.cert.pem"))?);
    assert_eq!(key, std::fs::read(holder.join("holder.key.pem"))?);
    server.stop().await?;
    Ok(())
}
