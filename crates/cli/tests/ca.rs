mod pki_support;

use cli::Exit;
use kernel::ca::{CertificateAuthority, CsrPem, Principal, SignAuthority};
use pki_support::{Result, directory, run, text};
use std::os::unix::fs::{PermissionsExt, symlink};
use tokio_util::sync::CancellationToken;
use x509_parser::prelude::FromDer;

#[tokio::test]
async fn operator_commands_initialize_export_print_once_revoke_and_report() -> Result {
    let home = directory()?;
    let root_key = home.path().join("offline root.pem");
    let state = home.path().join(".rivethub/ca");
    let initialized = run(
        home.path(),
        &["ca", "init", "--root-key-out", text(&root_key)?],
    )
    .await?;
    assert_eq!(initialized.exit, Exit::Ok, "{}", initialized.stderr);
    let principal: Principal = "spiffe://rivet.mesh/node/node-a".parse()?;
    let token = run(home.path(), &["ca", "token", &principal.spiffe_uri()?]).await?;
    assert_eq!(token.exit, Exit::Ok);
    assert_eq!(token.stderr, "");
    assert_eq!(token.stdout.len(), 65);
    assert_eq!(token.stdout.lines().count(), 1);
    let authority = pki::Pki::open(state.clone(), CancellationToken::new()).await?;
    let mut params = rcgen::CertificateParams::default();
    params.subject_alt_names = vec![rcgen::SanType::URI(principal.spiffe_uri()?.try_into()?)];
    let key = rcgen::KeyPair::generate()?;
    let csr = params.serialize_request(&key)?.pem()?;
    let certificate = authority
        .sign(
            CsrPem(csr.clone()),
            SignAuthority::Token(kernel::ca::EnrollmentToken(token.stdout.trim().to_owned())),
        )
        .await?;
    assert!(
        authority
            .sign(
                CsrPem(csr),
                SignAuthority::Token(kernel::ca::EnrollmentToken(token.stdout.trim().to_owned()))
            )
            .await
            .is_err()
    );
    let der = pki::certificate(&certificate.leaf)?;
    let (_, certificate) = x509_parser::certificate::X509Certificate::from_der(&der)?;
    let serial = certificate.raw_serial_as_string().replace(':', "");
    drop(authority);
    for target in [&serial, &principal.spiffe_uri()?] {
        assert_eq!(
            run(home.path(), &["ca", "revoke", target]).await?.exit,
            Exit::Ok
        );
    }
    assert_eq!(
        run(home.path(), &["ca", "token", &principal.spiffe_uri()?])
            .await?
            .exit,
        Exit::Failure
    );
    let published = home.path().join("compatibility.crl.pem");
    let crl = run(home.path(), &["ca", "crl", "--crl-out", text(&published)?]).await?;
    assert_eq!(crl.exit, Exit::Ok);
    assert_eq!(crl.stdout, std::fs::read_to_string(published)?);
    assert!(crl.stdout.starts_with("-----BEGIN X509 CRL-----"));
    let status = run(home.path(), &["ca", "status"]).await?;
    assert_eq!(status.exit, Exit::Ok);
    assert!(status.stdout.contains("RivetHub intermediate"));
    for output in [initialized.stdout, crl.stdout, status.stdout] {
        assert!(!output.contains(token.stdout.trim()));
        assert!(!output.contains("PRIVATE KEY"));
    }
    Ok(())
}

#[tokio::test]
async fn import_is_one_time_preserves_source_and_existing_chain() -> Result {
    let home = directory()?;
    let source = home.path().join("legacy layout");
    let initial = home.path().join("original");
    let root_export = home.path().join("offline.pem");
    let authority =
        pki::Pki::initialize(initial.clone(), root_export, CancellationToken::new()).await?;
    let expected = authority.chain().await?;
    drop(authority);
    for part in ["root/certs", "intermediate/certs", "intermediate/private"] {
        std::fs::create_dir_all(source.join(part))?;
    }
    for (from, to) in [
        ("root.cert.pem", "root/certs/ca.cert.pem"),
        (
            "intermediate.cert.pem",
            "intermediate/certs/intermediate.cert.pem",
        ),
        (
            "intermediate.key.pem",
            "intermediate/private/intermediate.key.pem",
        ),
    ] {
        std::fs::copy(initial.join(from), source.join(to))?;
    }
    std::fs::write(
        source.join("intermediate/index.txt"),
        "V\t351001000000Z\t\t1001\tunknown\t/CN=legacy-node\n",
    )?;
    std::fs::write(source.join("intermediate/serial"), "1002\n")?;
    let before = std::fs::read(source.join("intermediate/private/intermediate.key.pem"))?;
    assert_eq!(
        run(home.path(), &["ca", "import", text(&source)?])
            .await?
            .exit,
        Exit::Ok
    );
    let authority =
        pki::Pki::open(home.path().join(".rivethub/ca"), CancellationToken::new()).await?;
    assert_eq!(authority.chain().await?, expected);
    drop(authority);
    assert_eq!(
        run(home.path(), &["ca", "revoke", "1001"]).await?.exit,
        Exit::Ok
    );
    assert_eq!(
        run(home.path(), &["ca", "import", text(&source)?])
            .await?
            .exit,
        Exit::Failure
    );
    assert_eq!(
        before,
        std::fs::read(source.join("intermediate/private/intermediate.key.pem"))?
    );
    Ok(())
}

#[tokio::test]
async fn provisioning_refuses_traversal_symlinks_and_root_export_into_state() -> Result {
    let home = directory()?;
    let state = home.path().join("state");
    let outside = home.path().join("outside");
    std::fs::create_dir(&outside)?;
    symlink(&outside, &state)?;
    let root = home.path().join("root.pem");
    assert_eq!(
        run(
            home.path(),
            &[
                "ca",
                "--state",
                text(&state)?,
                "init",
                "--root-key-out",
                text(&root)?
            ]
        )
        .await?
        .exit,
        Exit::Failure
    );
    std::fs::remove_file(&state)?;
    assert_eq!(
        run(
            home.path(),
            &[
                "ca",
                "--state",
                text(&state)?,
                "init",
                "--root-key-out",
                text(&state.join("root.pem"))?
            ]
        )
        .await?
        .exit,
        Exit::Failure
    );
    assert_eq!(
        run(
            home.path(),
            &[
                "ca",
                "--state",
                text(&state.join("../escape"))?,
                "init",
                "--root-key-out",
                text(&root)?
            ]
        )
        .await?
        .exit,
        Exit::Failure
    );
    assert!(!root.exists());
    Ok(())
}

#[test]
fn principal_design_scenarios_are_typed_and_replayed() -> Result {
    #[derive(serde::Deserialize)]
    struct Case {
        uri: String,
        principal: Option<Principal>,
    }
    let cases: Vec<Case> = serde_json::from_str(&std::fs::read_to_string(testkit::golden_path(
        env!("CARGO_MANIFEST_DIR"),
        "tests/fixtures/pki/principals.json",
    ))?)?;
    for case in cases {
        match case.principal {
            Some(expected) => {
                assert_eq!(case.uri.parse::<Principal>()?, expected);
                assert_eq!(expected.spiffe_uri()?, case.uri);
            }
            None => assert!(case.uri.parse::<Principal>().is_err()),
        }
    }
    Ok(())
}

#[tokio::test]
async fn initialization_exports_root_once_and_keeps_only_intermediate_in_state() -> Result {
    let home = directory()?;
    let root_key = home.path().join("offline root.pem");
    let state = home.path().join(".rivethub/ca");
    assert_eq!(
        run(
            home.path(),
            &["ca", "init", "--root-key-out", text(&root_key)?]
        )
        .await?
        .exit,
        Exit::Ok
    );
    assert_eq!(
        std::fs::metadata(&root_key)?.permissions().mode() & 0o777,
        0o600
    );
    assert!(!state.join("root.key.pem").exists());
    assert_eq!(
        std::fs::metadata(state.join("intermediate.key.pem"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        run(
            home.path(),
            &["ca", "init", "--root-key-out", text(&root_key)?]
        )
        .await?
        .exit,
        Exit::Failure
    );
    Ok(())
}
