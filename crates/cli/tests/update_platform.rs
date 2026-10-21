#![cfg(not(all(
    any(target_os = "linux", target_os = "macos"),
    any(target_arch = "x86_64", target_arch = "aarch64")
)))]

use std::ffi::OsStr;

use cli::Exit;
use cli::service::Scope;
use cli::update::{Channel, UpdateArgs, Updater};
use config::Environment;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn update_refuses_other_platforms_before_touching_the_installation()
-> Result<(), Box<dyn std::error::Error>> {
    let home = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR"))?;
    let binary = home.path().join("rh");
    std::fs::write(&binary, "old runtime")?;
    let updater = Updater::new(
        binary.clone(),
        "http://127.0.0.1:9/releases/".parse()?,
        Scope::User,
    );
    let args = UpdateArgs {
        channel: Channel::Stable,
        version: None,
        no_restart: false,
        mesh: false,
        ssh_user: None,
    };
    let env = Environment::from_vars([(OsStr::new("HOME"), home.path().as_os_str())]);
    let report = updater.run(&args, &env, &CancellationToken::new()).await;
    assert_ne!(report.exit, Exit::Ok);
    assert!(
        report
            .stderr
            .contains("update requires Linux x86_64 or aarch64, or macOS"),
        "{}",
        report.stderr
    );
    assert_eq!(std::fs::read(&binary)?, b"old runtime");
    Ok(())
}
