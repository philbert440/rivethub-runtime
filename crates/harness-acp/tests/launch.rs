use std::path::Path;

use config::Config;
use harness_acp::AcpCommand;
use harness_acp::AcpError;
use harness_acp::AcpLaunch;
use harness_acp::ChatUnsupportedReason;
use harness_acp::LaunchTable;
use harness_acp::UnverifiedReason;
use wire::harness::HarnessId;

#[test]
fn terminal_only_harnesses_refuse_chat_with_the_recorded_reason() {
    let table = LaunchTable::from_config(&Config::default());
    let pi = table.launch(HarnessId::Pi).expect_err("pi refuses chat");
    assert!(matches!(
        pi,
        AcpError::ChatUnsupported {
            reason: ChatUnsupportedReason::TerminalOnly,
            ..
        }
    ));
    let cursor = table
        .launch(HarnessId::Cursor)
        .expect_err("cursor refuses chat");
    assert!(matches!(
        cursor,
        AcpError::ChatUnsupported {
            reason: ChatUnsupportedReason::TerminalOnly,
            ..
        }
    ));
    let cowork = table
        .launch(HarnessId::Cowork)
        .expect_err("cowork refuses chat");
    assert!(matches!(
        cowork,
        AcpError::ChatUnsupported {
            reason: ChatUnsupportedReason::CaptureOnly,
            ..
        }
    ));
}

#[test]
fn grok_launches_the_argv_verified_by_initialize() {
    let table = LaunchTable::from_config(&Config::default());
    let launch = table.launch(HarnessId::GrokBuild).expect("launch");
    let AcpLaunch::Native(command) = launch else {
        panic!("grok launches natively");
    };
    assert_eq!(command.program, Path::new("grok"));
    assert_eq!(command.args, vec!["agent".to_owned(), "stdio".to_owned()]);
    let verified = table
        .verified_command(HarnessId::GrokBuild)
        .expect("verified");
    assert_eq!(verified.args, vec!["agent".to_owned(), "stdio".to_owned()]);
}

#[test]
fn harnesses_that_are_not_installed_stay_unverified() {
    let table = LaunchTable::from_config(&Config::default());
    let defaults = [
        (HarnessId::Opencode, "opencode", vec!["acp".to_owned()]),
        (HarnessId::Codex, "codex-acp", Vec::new()),
        (HarnessId::Hermes, "hermes", Vec::new()),
        (HarnessId::KimiCode, "kimi", Vec::new()),
        (HarnessId::QwenCode, "qwen", Vec::new()),
    ];
    for (harness, program, args) in defaults {
        let AcpLaunch::Unverified { command, reason } = table.launch(harness).expect("entry")
        else {
            panic!("{harness} is an unverified entry");
        };
        assert_eq!(reason, UnverifiedReason::NotInstalled);
        assert_eq!(command.program, Path::new(program));
        assert_eq!(command.args, args);
        let error = table
            .verified_command(harness)
            .expect_err("unverified invocations are refused");
        let message = error.to_string();
        assert!(message.contains("not verified"), "{message}");
        assert!(message.contains("not installed"), "{message}");
    }
}

#[test]
fn config_binary_and_cwd_override_the_launch() {
    let document = "
tasks:
  harnesses:
    opencode:
      binary: /home/user/bin/opencode
      cwd: /home/user/workspace
";
    let config: Config = serde_yaml_ng::from_str(document).expect("config parses");
    let table = LaunchTable::from_config(&config);
    let launch = table.launch(HarnessId::Opencode).expect("launch");
    assert_eq!(
        launch.command().program,
        Path::new("/home/user/bin/opencode")
    );
    assert_eq!(
        launch.command().working_dir,
        Some(std::path::PathBuf::from("/home/user/workspace"))
    );
    assert_eq!(launch.command().args, vec!["acp".to_owned()]);
}

#[test]
fn a_configured_binary_does_not_verify_an_unverified_invocation() {
    let document = "
tasks:
  harnesses:
    hermes:
      binary: /home/user/bin/hermes
";
    let config: Config = serde_yaml_ng::from_str(document).expect("config parses");
    let table = LaunchTable::from_config(&config);
    let AcpLaunch::Unverified { command, reason } = table.launch(HarnessId::Hermes).expect("entry")
    else {
        panic!("a configured path does not verify an invocation that is not installed");
    };
    assert_eq!(reason, UnverifiedReason::NotInstalled);
    assert_eq!(command.program, Path::new("/home/user/bin/hermes"));
    assert_eq!(command.args, Vec::<String>::new());
    let error = table
        .verified_command(HarnessId::Hermes)
        .expect_err("configuration does not verify the invocation");
    assert!(error.to_string().contains("not installed"));
}

#[test]
fn an_explicit_table_serves_exactly_its_entries() {
    let table = LaunchTable::from_entries([(
        HarnessId::Opencode,
        AcpLaunch::Native(AcpCommand {
            program: std::path::PathBuf::from("/home/user/bin/fake_agent"),
            args: vec!["acp".to_owned()],
            working_dir: None,
        }),
    )]);
    assert!(table.launch(HarnessId::Opencode).is_ok());
    assert!(table.verified_command(HarnessId::Opencode).is_ok());
    let missing = table
        .launch(HarnessId::GrokBuild)
        .expect_err("not in the table");
    assert!(matches!(
        missing,
        AcpError::Unconfigured {
            harness: HarnessId::GrokBuild
        }
    ));
}

#[test]
fn claude_uses_the_current_acp_adapter() {
    let table = LaunchTable::from_config(&Config::default());
    let launch = table
        .launch(HarnessId::ClaudeCode)
        .expect("Claude launcher");
    assert!(matches!(launch, AcpLaunch::Adapter(_)));
    assert_eq!(
        table
            .verified_command(HarnessId::ClaudeCode)
            .expect("known invocation")
            .program,
        Path::new("claude-agent-acp")
    );
}

#[test]
fn configured_binary_paths_are_not_trimmed() {
    let config: Config = serde_yaml_ng::from_str(
        "tasks:\n  harnesses:\n    claude-code:\n      binary: ' /path with spaces/adapter '\n",
    )
    .expect("config");
    let command = LaunchTable::from_config(&config)
        .verified_command(HarnessId::ClaudeCode)
        .expect("command");
    assert_eq!(command.program, Path::new(" /path with spaces/adapter "));
}
