mod pki_support;

use cli::Exit;
use pki_support::{Result, directory, run, text};
use std::os::unix::fs::{PermissionsExt, symlink};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn holder_refuses_permissive_linked_and_symlinked_private_keys() -> Result {
    let home = directory()?;
    let ca = home.path().join("ca");
    let authority = pki::Pki::initialize(
        ca.clone(),
        home.path().join("root.pem"),
        CancellationToken::new(),
    )
    .await?;
    drop(authority);
    let holder = pki::files::directory(&home.path().join("holder"), true)?;
    let key = rcgen::KeyPair::generate()?.serialize_pem();
    let key_file = holder.join("holder.key.pem");
    let token = home.path().join("token");
    std::fs::write(&token, "1".repeat(64))?;
    let root = ca.join("root.cert.pem");
    let args = [
        "cert",
        "renew",
        "--state",
        text(&holder)?,
        "--principal",
        "spiffe://rivet.mesh/node/node-a",
        "--ca-url",
        "https://127.0.0.1:1",
        "--trust-root",
        text(&root)?,
        "--token-file",
        text(&token)?,
        "--once",
    ];
    std::fs::write(&key_file, &key)?;
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o644))?;
    let report = run(home.path(), &args).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("0600"));
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600))?;
    std::fs::hard_link(&key_file, home.path().join("linked.key"))?;
    let report = run(home.path(), &args).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("one hard link"));
    std::fs::remove_file(&key_file)?;
    symlink(home.path().join("linked.key"), &key_file)?;
    let report = run(home.path(), &args).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("escapes") || report.stderr.contains("symlink"));
    assert!(!holder.join("holder.cert.pem").exists());
    Ok(())
}

#[tokio::test]
async fn holder_refuses_plaintext_missing_tokens_and_invalid_token_content_without_echo() -> Result
{
    let home = directory()?;
    let ca = home.path().join("ca");
    let authority = pki::Pki::initialize(
        ca.clone(),
        home.path().join("root.pem"),
        CancellationToken::new(),
    )
    .await?;
    drop(authority);
    let root = ca.join("root.cert.pem");
    let args = [
        "cert",
        "renew",
        "--principal",
        "spiffe://rivet.mesh/node/node-a",
        "--ca-url",
        "http://127.0.0.1:1",
        "--trust-root",
        text(&root)?,
        "--once",
    ];
    let report = run(home.path(), &args).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("HTTPS"));
    let mut args = args;
    args[5] = "https://127.0.0.1:1";
    let report = run(home.path(), &args).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("--token-file"));
    let token = home.path().join("token");
    std::fs::write(&token, "do-not-echo-this-invalid-secret")?;
    let mut args = args.to_vec();
    args.extend(["--token-file", text(&token)?]);
    let report = run(home.path(), &args).await?;
    assert_eq!(report.exit, Exit::Failure);
    assert!(!report.stderr.contains("do-not-echo"));
    assert_eq!(report.stdout, "");
    Ok(())
}
