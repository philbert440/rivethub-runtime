use std::os::unix::process::ExitStatusExt;
mod support;

use cli::Exit;
use std::time::Duration;
use support::{config, environment, home, run};

#[tokio::test]
async fn stop_handles_missing_stale_and_live_pids() {
    let home = home();
    let env = environment(&home);
    assert_eq!(
        run(&["stop"], &env).await.stdout,
        "No running instance found (no PID file).\n"
    );
    config(&home, "");
    let mut child = tokio::process::Command::new("cat")
        .stdin(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("child awaiting stdin");
    let pid = child.id().expect("child PID");
    let file = home.path().join(".rivethub/rivethub.pid");
    std::fs::write(&file, format!("{pid}\n")).expect("pid file");
    let status = run(&["status"], &env).await;
    assert_eq!(status.exit, Exit::Ok);
    assert!(status.stdout.contains(&format!("running (PID {pid};")));
    let stopped = run(&["stop"], &env).await;
    assert_eq!(stopped.exit, Exit::Ok);
    assert_eq!(stopped.stdout, format!("Sent SIGTERM to PID {pid}\n"));
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("SIGTERM deadline")
        .expect("reap child");
    assert_eq!(status.signal(), Some(15));
    let stale = run(&["stop"], &env).await;
    assert_eq!(stale.exit, Exit::Ok);
    assert_eq!(
        stale.stdout,
        "PID file exists but process is not running.\n"
    );
}

#[tokio::test]
async fn invalid_pid_values_and_escape_links_are_refused() {
    let home = home();
    let env = environment(&home);
    config(&home, "");
    let file = home.path().join(".rivethub/rivethub.pid");
    for value in [
        "0",
        "1",
        "-1",
        "-20",
        "2147483648",
        "3 4",
        "4.5",
        "",
        "not a pid",
    ] {
        std::fs::write(&file, value).expect("pid file");
        assert_eq!(run(&["stop"], &env).await.exit, Exit::Failure, "{value}");
        assert_eq!(
            run(&["status"], &env).await.exit,
            Exit::Ok,
            "status always succeeds"
        );
    }
    std::fs::remove_file(&file).expect("remove pid");
    let sibling = home.path().join(".rivethub-sibling");
    std::fs::create_dir(&sibling).expect("sibling");
    std::fs::write(sibling.join("pid"), std::process::id().to_string()).expect("pid");
    std::os::unix::fs::symlink("../.rivethub-sibling/pid", &file).expect("escape link");
    assert_eq!(run(&["stop"], &env).await.exit, Exit::Failure);
    std::fs::remove_file(&file).expect("remove link");
    let local = file.with_file_name("local.pid");
    let mut child = tokio::process::Command::new("cat")
        .stdin(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("child");
    std::fs::write(&local, child.id().expect("pid").to_string()).expect("local pid");
    std::os::unix::fs::symlink("local.pid", &file).expect("contained link");
    assert_eq!(run(&["stop"], &env).await.exit, Exit::Ok);
    tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("deadline")
        .expect("child exit");
}

#[tokio::test]
async fn pid_files_must_be_regular_files() {
    let home = home();
    let env = environment(&home);
    config(&home, "");
    let file = home.path().join(".rivethub/rivethub.pid");
    nix::unistd::mkfifo(
        &file,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .expect("FIFO");
    let report = tokio::time::timeout(Duration::from_secs(30), run(&["stop"], &env))
        .await
        .expect("FIFO refusal completes without a writer");
    assert_eq!(report.exit, Exit::Failure);
    assert!(report.stderr.contains("PID"), "{}", report.stderr);
}

#[tokio::test]
async fn a_state_directory_link_cannot_redirect_stop_outside_the_state_root() {
    let home = home();
    let env = environment(&home);
    let sibling = home.path().join(".rivethub-sibling");
    std::fs::create_dir(&sibling).expect("sibling");
    std::fs::write(sibling.join("rivethub.pid"), std::process::id().to_string())
        .expect("external PID");
    std::os::unix::fs::symlink(&sibling, home.path().join(".rivethub")).expect("state link");
    assert_eq!(run(&["stop"], &env).await.exit, Exit::Failure);
}
