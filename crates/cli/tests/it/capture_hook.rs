use crate::support;

use cli::Exit;
use support::{config, environment, home, run};

const PROMPT: &str = r#"{"session_id":"55555555-5555-5555-5555-555555555555","cwd":"/work/demo","hook_event_name":"UserPromptSubmit","prompt":"list the files"}"#;
const TOOL: &str = r#"{"session_id":"55555555-5555-5555-5555-555555555555","cwd":"/work/demo","hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"ls"},"tool_use_id":"toolu_1"}"#;

fn now() -> Result<chrono::DateTime<chrono::Utc>, chrono::ParseError> {
    "2026-10-10T12:00:00Z".parse()
}

#[tokio::test]
async fn the_hook_never_fails_the_harness_without_a_config()
-> Result<(), Box<dyn std::error::Error>> {
    let home = home();
    let env = environment(&home);
    let report = cli::capture_hook::hook(&env, PROMPT, now()?).await;
    assert_eq!(report.exit, Exit::Ok);
    assert_eq!(report.stdout, "");
    assert!(
        report
            .stderr
            .starts_with("capture hook is not configured: "),
        "{}",
        report.stderr
    );
    Ok(())
}

#[tokio::test]
async fn an_unreachable_gateway_spools_the_batch() -> Result<(), Box<dyn std::error::Error>> {
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    let port = probe.local_addr()?.port();
    drop(probe);
    let home = home();
    let env = environment(&home);
    config(
        &home,
        &format!("gateway:\n  host: 127.0.0.1\n  port: {port}\n"),
    );
    let report = cli::capture_hook::hook(&env, TOOL, now()?).await;
    assert_eq!(report.exit, Exit::Ok);
    assert!(
        report.stderr.starts_with("capture request failed"),
        "{}",
        report.stderr
    );
    let spooled = std::fs::read_dir(home.path().join(".rivethub/capture-spool"))?.count();
    assert_eq!(spooled, 1);
    Ok(())
}

#[tokio::test]
async fn the_capture_command_lists_the_hook_subcommand() {
    let home = home();
    let env = environment(&home);
    let report = run(&["capture", "--help"], &env).await;
    assert_eq!(report.exit, Exit::Ok);
    assert!(
        report
            .stdout
            .contains("hook  Capture one harness hook payload read from stdin"),
        "{}",
        report.stdout
    );
}
