use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::time::Duration;

use config::provider::CliProvider;
use harness_acp::{AcpError, AcpHarnessAdapter, UnverifiedReason};
use kernel::harness::{HarnessAdapter, SessionEvent, SessionSpec, TurnInput};
use wire::harness::HarnessId;
use wire::session_id::SessionId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn launcher(path: &Path, session: &str) -> TestResult {
    let binary = env!("CARGO_BIN_EXE_fake-acp-agent").replace('\'', "'\"'\"'");
    std::fs::write(
        path,
        format!("#!/bin/sh\nexec '{binary}' --session-id '{session}' --steps text:Hi\n"),
    )?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[tokio::test]
async fn configured_headless_binary_and_directory_reach_the_acp_process() -> TestResult {
    for harness in [
        HarnessId::ClaudeCode,
        HarnessId::Codex,
        HarnessId::GrokBuild,
        HarnessId::Opencode,
        HarnessId::Hermes,
        HarnessId::KimiCode,
    ] {
        let home = tempfile::tempdir()?;
        let program = home.path().join("launcher with spaces");
        let native = match harness {
            HarnessId::Opencode => "ses_fake0123456789abcdefgh",
            HarnessId::GrokBuild => "0199299a-1a98-7000-8000-000000000001",
            HarnessId::Hermes => "20261015_120000_a1b2c3",
            HarnessId::KimiCode => "session_00000000-0000-4000-8000-000000000001",
            HarnessId::ClaudeCode | HarnessId::Codex => "00000000-0000-4000-8000-000000000001",
            HarnessId::Pi | HarnessId::QwenCode | HarnessId::Cursor | HarnessId::Cowork => {
                panic!("not a CLI provider")
            }
        };
        launcher(&program, native)?;
        let provider = CliProvider {
            binary: Some("launcher with spaces".into()),
            cwd: Some(home.path().into()),
            ..CliProvider::default()
        };
        let adapter = AcpHarnessAdapter::for_provider(
            &config::Config::default(),
            harness,
            &provider,
            vec![home.path().into()],
        )
        .await?;
        let session = adapter
            .new_session(SessionSpec {
                session: SessionId::new(harness, native),
                harness,
                model: Some("grok-5".into()),
                effort: Some(wire::stream::ThinkingLevel::Low),
                working_dir: None,
            })
            .await?;
        let mut events = adapter.events(&session).await?;
        tokio::time::timeout(
            Duration::from_secs(5),
            adapter.prompt(&session, TurnInput::Text("Say hi.".into())),
        )
        .await??;
        let response = tokio::time::timeout(Duration::from_secs(5), async {
            let mut text = String::new();
            while let Some(event) = events.recv().await {
                if let SessionEvent::AgentMessageChunk { text: delta } = event {
                    text.push_str(&delta);
                } else if event == SessionEvent::TurnEnded {
                    return text;
                } else {
                    panic!("unexpected event: {event:?}");
                }
            }
            panic!("turn ended without completion");
        })
        .await?;
        assert_eq!(response, "model=grok-5thought_level=lowHi");
        adapter.release(&session).await?;
    }
    Ok(())
}

#[tokio::test]
async fn absent_and_non_executable_programs_fail_before_any_turn() -> TestResult {
    let home = tempfile::tempdir()?;
    let program = home.path().join("launcher");
    for present in [false, true] {
        if present {
            std::fs::write(&program, "not executable")?;
        }
        let provider = CliProvider {
            binary: Some(program.display().to_string()),
            ..CliProvider::default()
        };
        let error = AcpHarnessAdapter::for_provider(
            &config::Config::default(),
            HarnessId::ClaudeCode,
            &provider,
            Vec::new(),
        )
        .await
        .expect_err("unavailable executable");
        assert!(matches!(
            error,
            AcpError::UnverifiedLaunch {
                harness: HarnessId::ClaudeCode,
                reason: UnverifiedReason::NotInstalled,
                ..
            }
        ));
    }
    Ok(())
}

#[tokio::test]
async fn an_installed_program_without_acp_is_not_a_launcher() -> TestResult {
    let home = tempfile::tempdir()?;
    let program = home.path().join("launcher");
    std::fs::write(&program, "#!/bin/sh\nexit 0\n")?;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))?;
    for harness in [HarnessId::Hermes, HarnessId::KimiCode] {
        let provider = CliProvider {
            binary: Some(program.display().to_string()),
            cwd: Some(home.path().into()),
            ..CliProvider::default()
        };
        let error = AcpHarnessAdapter::for_provider(
            &config::Config::default(),
            harness,
            &provider,
            Vec::new(),
        )
        .await
        .expect_err("unverified invocation");
        assert!(matches!(
            error,
            AcpError::Closed { .. } | AcpError::Protocol { .. } | AcpError::Launch { .. }
        ));
    }
    Ok(())
}
