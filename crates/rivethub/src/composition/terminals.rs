use std::path::PathBuf;
use std::sync::Arc;

use config::{Config, Environment};
use gateway::sessions::SessionRegistry;
use gateway::term::{LaunchCommand, TerminalOptions, TerminalService};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::ComposeError;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Roster {
    default: String,
    commands: Vec<Command>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    id: String,
    label: String,
    argv: Vec<String>,
    cwd: PathBuf,
    harness: Option<wire::harness::HarnessId>,
    session: bool,
}

pub(super) async fn build(
    config: &Config,
    env: &Environment,
    sessions: Arc<SessionRegistry>,
    shutdown: &CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) -> Result<Arc<TerminalService>, ComposeError> {
    let gateway = config.gateway.as_ref().ok_or_else(|| {
        ComposeError::Invalid("gateway settings are required for terminals".into())
    })?;
    let root = gateway
        .files_root
        .clone()
        .or_else(|| {
            config
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.workspace.clone())
        })
        .ok_or_else(|| ComposeError::Invalid("terminal filesystem root is required".into()))?;
    let path = gateway
        .commands_file
        .as_ref()
        .ok_or_else(|| ComposeError::Invalid("commands_file is required for terminals".into()))?;
    let roster: Roster = serde_json::from_slice(&tokio::fs::read(path).await?)?;
    let settings = terminal::TerminalSettings::from_config(
        config,
        env,
        env.node_name(&crate::service::host_name()),
    );
    let max_ptys = settings.limits.max_ptys;
    let paths = match env.get("PATH") {
        Some(path) => std::env::split_paths(path).collect::<Vec<_>>(),
        None => Vec::new(),
    };
    let plan = terminal::mux::resolve(
        config::TermMuxChoice {
            setting: config::TermMuxSetting::Herdr,
            ignored: None,
        },
        &paths,
        env.home().map(std::path::Path::new),
        &config::Paths::new(env).state_dir(),
        gateway.port,
    )
    .await;
    if let terminal::MuxPlan::Unavailable(unavailable) = &plan {
        return Err(unavailable.reason.to_error().into());
    }
    let mux = Arc::new(terminal::TerminalManager::new(settings, plan));
    mux.run_tracked(shutdown.child_token(), tasks);
    let commands = roster
        .commands
        .into_iter()
        .map(|command| LaunchCommand {
            id: command.id,
            label: command.label,
            argv: command.argv,
            cwd: command.cwd,
            harness: command.harness,
            session: command.session,
        })
        .collect();
    let service = TerminalService::new(
        mux,
        sessions,
        TerminalOptions {
            root,
            default: roster.default,
            commands,
            max_ptys,
        },
    )
    .await?
    .with_shutdown(shutdown.child_token())
    .with_harness(Arc::new(harness_acp::AcpHarnessAdapter::new(config)));
    Ok(Arc::new(service))
}
