#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use std::path::PathBuf;
use std::time::Duration;

use libtest_mimic::Arguments;
use libtest_mimic::Failed;
use libtest_mimic::Trial;
use libtest_mimic::run;
use terminal::TerminalState;
use testkit::recording;

const RECORDING: &str = "term/node-pty.json";

#[derive(serde::Deserialize)]
struct Recording {
    cases: Vec<CaseFile>,
}

#[derive(serde::Deserialize)]
struct CaseFile {
    name: String,
    argv: Vec<String>,
    #[serde(rename = "parentKill")]
    parent_kill: Option<String>,
    #[serde(rename = "onExit")]
    on_exit: OnExit,
    #[serde(default)]
    output: String,
}

#[derive(serde::Deserialize)]
struct OnExit {
    #[serde(rename = "exitCode")]
    exit_code: u32,
    signal: u32,
}

struct Case {
    name: String,
    argv: Vec<String>,
    parent_kill: Option<String>,
    expected_code: u32,
    expected_output: String,
}

fn cases() -> Result<Vec<Case>, Failed> {
    let text = std::fs::read_to_string(recording(RECORDING))
        .map_err(|error| Failed::from(format!("vendored recording unreadable: {error}")))?;
    let parsed: Recording = serde_json::from_str(&text)
        .map_err(|error| Failed::from(format!("recording json: {error}")))?;
    parsed
        .cases
        .into_iter()
        .map(|entry| {
            if entry.on_exit.signal != 0 && entry.on_exit.exit_code != 0 {
                return Err(Failed::from(format!(
                    "{}: the recording pins signal deaths as exit code 0",
                    entry.name
                )));
            }
            Ok(Case {
                name: entry.name,
                argv: entry.argv,
                parent_kill: entry.parent_kill,
                expected_code: entry.on_exit.exit_code,
                expected_output: entry.output,
            })
        })
        .collect()
}

fn kill_signal(parent_kill: &str) -> nix::sys::signal::Signal {
    match parent_kill {
        "SIGTERM" => nix::sys::signal::Signal::SIGTERM,
        "SIGKILL" => nix::sys::signal::Signal::SIGKILL,
        _ => nix::sys::signal::Signal::SIGHUP,
    }
}

fn silent_child(argv: Vec<String>) -> Result<Vec<String>, Failed> {
    if argv.iter().any(|part| part.ends_with("acme-tty.mjs")) {
        let program = argv
            .first()
            .ok_or_else(|| Failed::from("the recorded argv is empty"))?
            .clone();
        return Ok(vec![
            program,
            "-e".to_owned(),
            "setInterval(() => {}, 1000)".to_owned(),
        ]);
    }
    Ok(argv)
}

async fn wait_exit(
    manager: &terminal::TerminalManager,
    id: &kernel::terminal::TerminalId,
    name: &str,
) -> Result<terminal::TerminalRecord, Failed> {
    manager
        .wait_exited(id)
        .await
        .map_err(|error| Failed::from(format!("{name}: the exit wait failed: {error}")))
}

async fn replay(case: Case, node: Option<PathBuf>) -> Result<(), Failed> {
    let manager = common::direct_manager(&config::Environment::default());
    let session = "claude-code:00000000-0000-4000-8000-000000000300";
    let tmp = tempfile::tempdir().map_err(|error| Failed::from(error.to_string()))?;
    let tmp_path = tmp.path().to_str().ok_or("the tempdir is not utf8")?;
    let argv = if case.argv.first().is_some_and(|part| part.contains("node")) {
        let node = node.ok_or_else(|| {
            Failed::from("node is not installed on this machine; the recorded argv needs it")
        })?;
        let bin = tmp.path().join("bin");
        std::fs::create_dir_all(&bin).map_err(|error| Failed::from(error.to_string()))?;
        std::os::unix::fs::symlink(&node, bin.join("node"))
            .map_err(|error| Failed::from(error.to_string()))?;
        let substituted: Vec<String> = case
            .argv
            .iter()
            .map(|part| part.replace("{{TMP}}", tmp_path))
            .collect();
        silent_child(substituted)?
    } else {
        case.argv.clone()
    };
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    let id = manager
        .start(common::spec(session, tmp.path(), &borrowed)?)
        .await
        .map_err(|error| Failed::from(format!("{}: spawn: {error}", case.name)))?;
    if let Some(parent_kill) = &case.parent_kill {
        #[cfg_attr(
            dylint_lib = "rr_lints",
            expect(
                sleep,
                reason = "the child needs a real moment to register with the reaper before the signal arrives"
            )
        )]
        tokio::time::sleep(Duration::from_millis(200)).await;
        let pid = manager
            .list()
            .into_iter()
            .find(|record| record.id == id)
            .and_then(|record| record.pid)
            .ok_or_else(|| Failed::from(format!("{}: no pid to signal", case.name)))?;
        terminal::pty::signal(pid, kill_signal(parent_kill))
            .map_err(|error| Failed::from(format!("{}: {error}", case.name)))?;
    }
    let record = wait_exit(&manager, &id, &case.name).await?;
    if record.state
        != (TerminalState::Exited {
            code: case.expected_code,
        })
    {
        return Err(Failed::from(format!(
            "{}: expected exit code {}, got {:?}",
            case.name, case.expected_code, record.state
        )));
    }
    let attached = manager
        .attach_stream(&id)
        .map_err(|error| Failed::from(format!("{}: attach: {error}", case.name)))?;
    if attached.replay != case.expected_output.as_bytes() {
        return Err(Failed::from(format!(
            "{}: recorded output {:?}, got {:?}",
            case.name,
            String::from_utf8_lossy(case.expected_output.as_bytes()),
            String::from_utf8_lossy(&attached.replay)
        )));
    }
    Ok(())
}

fn replay_case(case: Case, node: Option<PathBuf>) -> Result<(), Failed> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| Failed::from(format!("dev test runtime: {error}")))?;
    runtime.block_on(replay(case, node))
}

fn main() {
    let args = Arguments::from_args();
    let node = common::program_on_path("node");
    match cases() {
        Ok(recorded) => {
            let mut trials = Vec::new();
            for case in recorded {
                let node = node.clone();
                trials.push(Trial::test(format!("node-pty {}", case.name), move || {
                    replay_case(case, node)
                }));
            }
            run(&args, trials).exit()
        }
        Err(error) => run(
            &args,
            vec![Trial::test("node-pty recording", move || {
                Err::<(), Failed>(error)
            })],
        )
        .exit(),
    }
}
