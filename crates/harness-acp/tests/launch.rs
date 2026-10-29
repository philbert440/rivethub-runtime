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
fn verified_acp_modes_launch_with_their_arguments() {
    let table = LaunchTable::from_config(&Config::default());
    let verified = [
        (HarnessId::Codex, "codex-acp", Vec::new()),
        (HarnessId::Opencode, "opencode", vec!["acp".to_owned()]),
        (HarnessId::Hermes, "hermes", vec!["acp".to_owned()]),
        (HarnessId::KimiCode, "kimi", vec!["acp".to_owned()]),
        (HarnessId::Omp, "omp", vec!["acp".to_owned()]),
        (HarnessId::Dsh, "dsh", vec!["acp".to_owned()]),
    ];
    for (harness, program, args) in verified {
        let command = table.verified_command(harness).expect("verified launch");
        assert_eq!(command.program, Path::new(program), "{harness}");
        assert_eq!(command.args, args, "{harness}");
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
fn an_explicit_table_serves_exactly_its_entries() {
    let table = LaunchTable::from_entries([(
        HarnessId::Opencode,
        AcpLaunch::Native(AcpCommand {
            program: std::path::PathBuf::from("/home/user/bin/fake_agent"),
            args: vec!["acp".to_owned()],
            working_dir: None,
            env: Vec::new(),
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
