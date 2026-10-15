mod agent;
mod checks;
mod gateway;
mod mcp;
mod worker;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use tokio::process::Command;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const DEAD_POSTGRES: &str = "postgres://127.0.0.1:1/none";

const OMITTED: &[(&str, &str)] = &[
    (
        "internet_search",
        "B7: harness owns web tools; removed by design",
    ),
    ("web_fetch", "B7: harness owns web tools; removed by design"),
    ("echo", "Product decision: not a sidecar tool"),
    (
        "skill_list",
        "Product decision: skills are not sidecar tools",
    ),
    (
        "skill_manage",
        "Product decision: skills are not sidecar tools",
    ),
    ("categorizer", "categorizer has no handler in this build"),
    ("wiki-builder", "wiki-builder has no handler in this build"),
];

fn provider_config(home: &Path, postgres: impl std::fmt::Display, extra: &str) -> String {
    format!(
        "runtime:\n  workspace: {}\n  default_agent: main\nagents:\n  main:\n    provider: p\n    model: m\nproviders:\n  p:\n    type: openai-compatible\n    base_url: http://127.0.0.1:1\n    model: m\nmemory:\n  postgres:\n    connection_string: {postgres}\n{extra}",
        home.display()
    )
}

fn write_state(home: &Path, body: &str) -> TestResult {
    let state = home.join(".rivethub");
    std::fs::create_dir_all(&state)?;
    std::fs::write(state.join("config.yaml"), body)?;
    Ok(())
}

async fn run_rh(home: &Path, args: &[&str]) -> TestResult<std::process::Output> {
    Ok(tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(env!("CARGO_BIN_EXE_rh"))
            .args(args)
            .env_clear()
            .env("HOME", home)
            .output(),
    )
    .await??)
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

async fn init_authority(home: &Path) -> TestResult {
    let offline = home.join("offline");
    std::fs::create_dir_all(&offline)?;
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        Command::new(env!("CARGO_BIN_EXE_rh"))
            .args(["ca", "init", "--root-key-out"])
            .arg(offline.join("root.key.pem"))
            .env_clear()
            .env("HOME", home)
            .output(),
    )
    .await??;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("ca init failed: {}", stderr(&output)).into())
    }
}

fn terminate(pid: u32) -> TestResult {
    let pid = i32::try_from(pid)?;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGTERM,
    )?;
    Ok(())
}

fn readme_skips(text: &str) -> BTreeMap<String, String> {
    let mut skips = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.trim().strip_prefix("- `") else {
            continue;
        };
        let Some((name, reason)) = rest.split_once("` — ") else {
            continue;
        };
        skips.insert(name.to_owned(), reason.trim().to_owned());
    }
    skips
}

#[test]
fn readme_names_every_omitted_scenario() {
    let readme =
        std::fs::read_to_string(testkit::recording("roots/README.md")).expect("roots README");
    let parsed = readme_skips(&readme);
    let expected: BTreeMap<_, _> = OMITTED
        .iter()
        .copied()
        .map(|(name, reason)| (name.to_owned(), reason.to_owned()))
        .collect();
    assert_eq!(parsed, expected);
    for reason in parsed.values() {
        assert_ne!(reason, "");
    }
}
