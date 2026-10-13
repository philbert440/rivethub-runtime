#![expect(
    unreachable_pub,
    reason = "the harness is included as a private module in every test binary"
)]
#![expect(
    dead_code,
    reason = "every test binary includes this module and uses a different subset"
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kernel::terminal::TerminalId;
use terminal::SessionKey;
use terminal::SpawnSpec;
use terminal::TerminalManager;
use terminal::TerminalSettings;
use terminal::TerminalState;
use tokio::sync::mpsc;

pub const HOSTNAME: &str = "gold-host";

pub fn env_from(vars: &[(&str, &str)]) -> config::Environment {
    config::Environment::from_vars(
        vars.iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    )
}

pub fn settings(env: &config::Environment) -> TerminalSettings {
    TerminalSettings::from_config(&config::Config::default(), env, HOSTNAME.to_owned())
}

pub fn direct_manager(env: &config::Environment) -> Arc<TerminalManager> {
    let manager = TerminalManager::new(settings(env), terminal::MuxPlan::Direct);
    let arc = Arc::new(manager);
    arc.run_tasks(tokio_util::sync::CancellationToken::new());
    arc
}

pub fn spec(session: &str, cwd: &Path, argv: &[&str]) -> Result<SpawnSpec, terminal::Error> {
    Ok(SpawnSpec {
        session: Some(SessionKey::new(session)?),
        roster_key: terminal::RosterKey::new("echo")?,
        argv: argv.iter().map(ToString::to_string).collect(),
        cwd: cwd.to_path_buf(),
        size: kernel::terminal::TermSize { cols: 80, rows: 24 },
        entry_env: BTreeMap::new(),
    })
}

pub async fn collect(rx: &mut mpsc::Receiver<Vec<u8>>, millis: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let window = Duration::from_millis(millis);
    while let Ok(chunk) = tokio::time::timeout(window, rx.recv()).await {
        match chunk {
            Some(bytes) => out.extend(bytes),
            None => break,
        }
    }
    out
}

pub async fn wait_for_exit(
    manager: &TerminalManager,
    id: &TerminalId,
) -> Result<terminal::TerminalRecord, String> {
    for _ in 0..400 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        if let Some(record) = manager.list().into_iter().find(|record| record.id == *id)
            && record.state != TerminalState::Running
        {
            return Ok(record);
        }
    }
    Err(format!("terminal {id} never exited"))
}

pub fn child_paths(path_var: &str) -> Vec<PathBuf> {
    path_var.split(':').map(PathBuf::from).collect()
}

pub fn program_on_path(program: &str) -> Option<PathBuf> {
    let path = config::Environment::from_process().text("PATH")?.to_owned();
    child_paths(&path)
        .into_iter()
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}
