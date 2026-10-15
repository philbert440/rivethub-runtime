#[path = "stepca_support/common.rs"]
mod support;

use cli::Exit;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use support::{Result, directory, run, text, token};

const PRINCIPAL: &str = "spiffe://rivet.mesh/node/node-a";

#[tokio::test]
async fn holder_refuses_permissive_linked_and_symlinked_private_keys() -> Result {
    let home = directory()?;
    let root = write_root(home.path())?;
    let state = home.path().join("holder");
    let tls = state.join("tls");
    std::fs::create_dir_all(&tls)?;
    std::fs::set_permissions(&tls, std::fs::Permissions::from_mode(0o700))?;
    let key = rcgen::KeyPair::generate()?.serialize_pem();
    let key_file = tls.join("holder.key.pem");
    let token_file = home.path().join("token");
    std::fs::write(&token_file, token(&["192.0.2.15", "node-a.example.test"])?)?;
    let args = holder_args(&state, &root, &token_file)?;
    std::fs::write(&key_file, &key)?;
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o644))?;
    let report = run(home.path(), &args).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("0600"), "{}", report.stderr);
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600))?;
    std::fs::hard_link(&key_file, home.path().join("linked.key"))?;
    let report = run(home.path(), &args).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("one hard link"), "{}", report.stderr);
    std::fs::remove_file(&key_file)?;
    symlink(home.path().join("linked.key"), &key_file)?;
    let report = run(home.path(), &args).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(
        report.stderr.contains("symlink") || report.stderr.contains("escapes"),
        "{}",
        report.stderr
    );
    assert!(!tls.join("holder.cert.pem").exists());
    Ok(())
}

#[tokio::test]
async fn refuses_plaintext_urls_missing_tokens_and_invalid_tokens_without_echo() -> Result {
    let home = directory()?;
    let root = write_root(home.path())?;
    let http = run(
        home.path(),
        &[
            "cert",
            "renew",
            "--principal",
            PRINCIPAL,
            "--ca-url",
            "http://127.0.0.1:1",
            "--trust-root",
            text(&root)?,
            "--once",
        ],
    )
    .await?;
    assert_eq!(http.exit, Exit::Failure);
    assert!(
        http.stderr.contains("scheme must be https"),
        "{}",
        http.stderr
    );
    assert!(!home.path().join(".rivethub/tls").exists());
    let missing = run(
        home.path(),
        &[
            "cert",
            "renew",
            "--principal",
            PRINCIPAL,
            "--ca-url",
            "https://127.0.0.1:1",
            "--trust-root",
            text(&root)?,
            "--once",
        ],
    )
    .await?;
    assert_eq!(missing.exit, Exit::Failure);
    assert!(
        missing.stderr.contains("--token-file"),
        "{}",
        missing.stderr
    );
    let token_file = home.path().join("token");
    std::fs::write(&token_file, "do-not-echo-this-invalid-secret")?;
    let bad = run_token(home.path(), &root, &token_file).await?;
    assert_eq!(bad.exit, Exit::Failure);
    assert!(!bad.stderr.contains("do-not-echo"), "{}", bad.stderr);
    std::fs::write(&token_file, "do-not-echo secret")?;
    let spaced = run_token(home.path(), &root, &token_file).await?;
    assert_eq!(spaced.exit, Exit::Failure);
    assert!(!spaced.stderr.contains("do-not-echo"), "{}", spaced.stderr);
    assert!(!home.path().join(".rivethub/tls").exists());
    Ok(())
}

#[tokio::test]
async fn refuses_a_token_uri_that_is_not_the_holder_principal() -> Result {
    let home = directory()?;
    let root = write_root(home.path())?;
    let token_file = home.path().join("token");
    std::fs::write(
        &token_file,
        token(&["spiffe://rivet.mesh/node/other", "node-a.example.test"])?,
    )?;
    let state = home.path().join("holder");
    let report = run(home.path(), &holder_args(&state, &root, &token_file)?).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("different URI"), "{}", report.stderr);
    assert!(!state.join("tls/holder.key.pem").exists());
    Ok(())
}

#[tokio::test]
async fn refuses_parent_segments_and_symlinked_state_directories() -> Result {
    let home = directory()?;
    let root = write_root(home.path())?;
    let token_file = home.path().join("token");
    std::fs::write(&token_file, token(&["192.0.2.15", "node-a.example.test"])?)?;
    let traversed = home.path().join("nested").join("..").join("holder");
    let report = run(home.path(), &holder_args(&traversed, &root, &token_file)?).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(
        report.stderr.contains("path traversal"),
        "{}",
        report.stderr
    );
    assert!(!home.path().join("holder").exists());
    let real = home.path().join("real");
    std::fs::create_dir(&real)?;
    let link = home.path().join("link");
    symlink(&real, &link)?;
    let report = run(
        home.path(),
        &holder_args(&link.join("holder"), &root, &token_file)?,
    )
    .await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("symlink"), "{}", report.stderr);
    assert!(!real.join("holder").exists());
    Ok(())
}

fn write_root(directory: &Path) -> Result<std::path::PathBuf> {
    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(vec!["root.example.test".to_owned()])?;
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    let certificate = params.self_signed(&key)?;
    let path = directory.join("root.pem");
    std::fs::write(&path, certificate.pem())?;
    Ok(path)
}

fn holder_args<'a>(state: &'a Path, root: &'a Path, token_file: &'a Path) -> Result<Vec<&'a str>> {
    Ok(vec![
        "cert",
        "renew",
        "--state",
        text(state)?,
        "--principal",
        PRINCIPAL,
        "--ca-url",
        "https://127.0.0.1:1",
        "--trust-root",
        text(root)?,
        "--token-file",
        text(token_file)?,
        "--once",
    ])
}

async fn run_token(home: &Path, root: &Path, token_file: &Path) -> Result<cli::Report> {
    run(
        home,
        &[
            "cert",
            "renew",
            "--principal",
            PRINCIPAL,
            "--ca-url",
            "https://127.0.0.1:1",
            "--trust-root",
            text(root)?,
            "--token-file",
            text(token_file)?,
            "--once",
        ],
    )
    .await
}
