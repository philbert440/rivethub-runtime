use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use kernel::terminal::TerminalId;
use terminal::SessionKey;
use terminal::SpawnSpec;
use terminal::TerminalManager;
use terminal::TerminalSettings;

pub(crate) const HOSTNAME: &str = "gold-host";

pub(crate) fn env_from(vars: &[(&str, &str)]) -> config::Environment {
    config::Environment::from_vars(
        vars.iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string())),
    )
}

pub(crate) fn settings(env: &config::Environment) -> TerminalSettings {
    TerminalSettings::from_config(&config::Config::default(), env, HOSTNAME.to_owned())
}

pub(crate) fn direct_manager(env: &config::Environment) -> Arc<TerminalManager> {
    let manager = TerminalManager::new(settings(env), terminal::MuxPlan::Direct);
    let arc = Arc::new(manager);
    let tasks = tokio_util::task::TaskTracker::new();
    arc.run_tracked(tokio_util::sync::CancellationToken::new(), &tasks);
    tasks.close();
    arc
}

pub(crate) fn spec(session: &str, cwd: &Path, argv: &[&str]) -> Result<SpawnSpec, terminal::Error> {
    Ok(SpawnSpec {
        session: Some(SessionKey::new(session)?),
        roster_key: terminal::RosterKey::new("echo")?,
        argv: argv.iter().map(ToString::to_string).collect(),
        cwd: cwd.to_path_buf(),
        size: kernel::terminal::TermSize { cols: 80, rows: 24 },
        entry_env: BTreeMap::new(),
    })
}

pub(crate) async fn until_output(
    stream: terminal::AttachStream,
    expected: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = stream.replay;
    let mut live = stream.live;
    tokio::time::timeout(Duration::from_secs(30), async {
        while !bytes
            .windows(expected.len())
            .any(|window| window == expected)
        {
            bytes.extend(
                live.as_mut()
                    .ok_or("terminal exited before expected output")?
                    .recv()
                    .await
                    .ok_or("output closed before expected output")?,
            );
        }
        Ok(bytes)
    })
    .await?
}

pub(crate) async fn wait_for_exit(
    manager: &TerminalManager,
    id: &TerminalId,
) -> Result<terminal::TerminalRecord, String> {
    tokio::time::timeout(Duration::from_secs(30), manager.wait_exited(id))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("terminal {id} exit wait failed: {error}"))
}
