#![cfg(test)]

use std::path::PathBuf;
use std::time::Duration;

use harness_acp::{AcpCommand, AcpHarnessAdapter, AcpLaunch, LaunchTable};
use kernel::harness::{HarnessAdapter, HarnessAdapterError, SessionSpec, TurnInput};
use wire::harness::HarnessId;
use wire::session_id::SessionId;

const WAIT: Duration = Duration::from_secs(60);

#[tokio::test]
async fn release_closes_the_owner_and_allows_resume_on_the_same_adapter() {
    let directory = tempfile::tempdir().expect("working directory");
    let adapter = AcpHarnessAdapter::with_launch_table(LaunchTable::from_entries([(
        HarnessId::Opencode,
        AcpLaunch::Native(AcpCommand {
            program: PathBuf::from(env!("CARGO_BIN_EXE_fake-acp-agent")),
            args: vec![
                "--session-id".into(),
                "ses_fake0123456789abcdefgh".into(),
                "--on-load".into(),
                "replay".into(),
            ],
            working_dir: Some(directory.path().to_path_buf()),
        }),
    )]));
    let session = tokio::time::timeout(
        WAIT,
        adapter.new_session(SessionSpec {
            session: SessionId::new(HarnessId::Opencode, "ses_fake0123456789abcdefgh"),
            harness: HarnessId::Opencode,
            model: None,
            effort: None,
            working_dir: Some(directory.path().to_path_buf()),
        }),
    )
    .await
    .expect("session initialization deadline")
    .expect("new session");
    let mut events = adapter.events(&session).await.expect("events");
    tokio::time::timeout(WAIT, adapter.release(&session))
        .await
        .expect("owner reap deadline")
        .expect("released and reaped");
    tokio::time::timeout(WAIT, async { while events.recv().await.is_some() {} })
        .await
        .expect("released owner closes its event stream");
    assert!(matches!(
        adapter
            .prompt(&session, TurnInput::Text("closed".into()))
            .await,
        Err(HarnessAdapterError::UnknownSession(_))
    ));
    tokio::time::timeout(WAIT, adapter.resume(&session))
        .await
        .expect("resume initialization deadline")
        .expect("resume");
    tokio::time::timeout(WAIT, adapter.release(&session))
        .await
        .expect("resumed owner reap deadline")
        .expect("release resumed owner");
}
