use std::path::Path;

use config::Config;
use harness_acp::AcpCommand;
use harness_acp::AcpError;
use harness_acp::AcpLaunch;
use harness_acp::ChatUnsupportedReason;
use harness_acp::LaunchTable;
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
fn opencode_launches_the_subcommand_verified_against_its_own_help() {
    let table = LaunchTable::from_config(&Config::default());
    let launch = table.launch(HarnessId::Opencode).expect("launch");
    let AcpLaunch::Native(command) = launch else {
        panic!("opencode launches natively");
    };
    assert_eq!(command.program, Path::new("opencode"));
    assert_eq!(command.args, vec!["acp".to_owned()]);
    let verified = table
        .verified_command(HarnessId::Opencode)
        .expect("verified");
    assert_eq!(verified.args, vec!["acp".to_owned()]);
}

#[test]
fn the_adapter_binaries_are_verified_endpoints_from_the_architecture() {
    let table = LaunchTable::from_config(&Config::default());
    let AcpLaunch::Adapter(claude) = table
        .launch(HarnessId::ClaudeCode)
        .expect("claude-code-acp")
    else {
        panic!("claude-code launches the claude-code-acp adapter");
    };
    assert_eq!(claude.program, Path::new("claude-code-acp"));
    assert_eq!(claude.args, Vec::<String>::new());
    let AcpLaunch::Adapter(codex) = table.launch(HarnessId::Codex).expect("codex-acp") else {
        panic!("codex launches the codex-acp adapter");
    };
    assert_eq!(codex.program, Path::new("codex-acp"));
    assert_eq!(codex.args, Vec::<String>::new());
}

#[test]
fn native_harnesses_without_a_recorded_invocation_are_unverified_entries() {
    let table = LaunchTable::from_config(&Config::default());
    let defaults = [
        (HarnessId::GrokBuild, "grok"),
        (HarnessId::Hermes, "hermes"),
        (HarnessId::KimiCode, "kimi"),
        (HarnessId::QwenCode, "qwen"),
    ];
    for (harness, default) in defaults {
        let AcpLaunch::Unverified(command) = table.launch(harness).expect("entry") else {
            panic!("{harness} is an unverified entry");
        };
        assert_eq!(command.program, Path::new(default));
        assert_eq!(command.args, Vec::<String>::new());
        let error = table
            .verified_command(harness)
            .expect_err("unverified invocations are refused");
        assert!(
            error.to_string().contains("not verified"),
            "typed refusal: {error}"
        );
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
    grok-build:
      binary: /home/user/bin/grok
";
    let config: Config = serde_yaml_ng::from_str(document).expect("config parses");
    let table = LaunchTable::from_config(&config);
    let AcpLaunch::Unverified(command) = table.launch(HarnessId::GrokBuild).expect("entry") else {
        panic!("a configured path is not a recorded invocation");
    };
    assert_eq!(command.program, Path::new("/home/user/bin/grok"));
    assert_eq!(command.args, Vec::<String>::new());
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
