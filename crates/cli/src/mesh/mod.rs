mod check;
mod issue;
mod remote;
mod store;

use std::path::PathBuf;

use clap::{Args, CommandFactory, Subcommand};
use config::Environment;
use serde::Serialize;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{Error, Exit, Report};

const FORGET_USAGE: &str = "Usage: rh mesh forget <node> [--force]\n";

#[derive(Debug, Args)]
pub struct MeshArgs {
    #[arg(long, global = true)]
    pub(crate) json: bool,
    #[arg(long, global = true)]
    pub(crate) ssh_user: Option<String>,
    #[command(subcommand)]
    command: Option<MeshCommand>,
}

#[derive(Debug, Subcommand)]
enum MeshCommand {
    #[command(about = "List mesh nodes")]
    List,
    #[command(about = "Probe mesh nodes")]
    Ping(PingArgs),
    #[command(about = "Enroll this node through step-ca and record it")]
    Enroll(EnrollArgs),
    #[command(about = "Merge a hub mesh registry")]
    Sync(SyncArgs),
    #[command(about = "Renew this node's holder certificate")]
    Renew(RenewArgs),
    #[command(about = "Join a mesh")]
    Join(JoinArgs),
    #[command(about = "Show the mesh registry")]
    Status,
    #[command(about = "Remove a mesh node")]
    Forget(ForgetArgs),
}

#[derive(Debug, Args)]
struct PingArgs {
    #[arg(long)]
    timeout: Option<u64>,
}

#[derive(Debug, Args)]
pub(super) struct EnrollArgs {
    target: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    advertise: Option<String>,
    #[arg(long, default_value = "rivethub-hub")]
    hub_cmd: String,
    #[arg(long)]
    ca_url: Option<Url>,
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long)]
    provisioner_password_file: Option<PathBuf>,
    #[arg(long)]
    roster: Option<PathBuf>,
    #[arg(long)]
    state: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(super) struct SyncArgs {
    target: Option<String>,
    #[arg(long, default_value = "rivethub-hub")]
    hub_cmd: String,
}

#[derive(Debug, Args)]
pub(super) struct RenewArgs {
    target: Option<String>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long, default_value = "rivethub-hub")]
    hub_cmd: String,
    #[arg(long)]
    ca_url: Option<Url>,
    #[arg(long)]
    root: Option<PathBuf>,
    #[arg(long)]
    state: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(super) struct JoinArgs {
    host: Option<String>,
    #[arg(long)]
    manual: bool,
    #[arg(long)]
    port: Option<u32>,
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    advertise: Option<String>,
}

#[derive(Debug, Args)]
struct ForgetArgs {
    target: Option<String>,
    #[arg(long)]
    force: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) enum EffectAction {
    Enrolled,
    Renewed,
    Synced,
    Joined,
    Forgot,
}

pub(super) struct Finished {
    pub(super) action: EffectAction,
    pub(super) node: Option<String>,
    pub(super) state: Option<PathBuf>,
    pub(super) mesh: PathBuf,
    pub(super) message: String,
    pub(super) warnings: Vec<String>,
    pub(super) json: bool,
}

#[derive(Serialize)]
struct Effect {
    action: EffectAction,
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    mesh: String,
    message: String,
}

pub fn help() -> String {
    let mut command = crate::tree::Cli::command();
    match command.find_subcommand_mut("mesh") {
        Some(mesh) => format!("{}\n", mesh.render_help()),
        None => String::from("rh mesh\n"),
    }
}

pub async fn run(
    args: MeshArgs,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    if shutdown.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if let Some(value) = args.ssh_user.as_deref()
        && let Some(report) = check::ssh_refusal(value)
    {
        return Ok(report);
    }
    settle(dispatch(args, env, shutdown).await)
}

async fn dispatch(
    args: MeshArgs,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    let MeshArgs {
        json,
        ssh_user,
        command,
    } = args;
    match command {
        None => Ok(Report::success(help())),
        Some(MeshCommand::List) => show(json, env, false).await,
        Some(MeshCommand::Status) => show(json, env, true).await,
        Some(MeshCommand::Ping(args)) => remote::ping(args.timeout, json, env, shutdown).await,
        Some(MeshCommand::Enroll(args)) => {
            issue::enroll(issue::Enrollment {
                args: &args,
                ssh_user,
                json,
                env,
                shutdown,
            })
            .await
        }
        Some(MeshCommand::Sync(args)) => {
            remote::sync(remote::SyncRequest {
                args: &args,
                json,
                env,
                shutdown,
            })
            .await
        }
        Some(MeshCommand::Renew(args)) => {
            issue::renew(issue::Renewal {
                args: &args,
                json,
                env,
                shutdown,
            })
            .await
        }
        Some(MeshCommand::Join(args)) => {
            remote::join(remote::JoinRequest {
                args: &args,
                ssh_user,
                json,
                env,
                shutdown,
            })
            .await
        }
        Some(MeshCommand::Forget(args)) => forget(args, json, env, shutdown).await,
    }
}

async fn show(json: bool, env: &Environment, status: bool) -> Result<Report, Error> {
    let path = store::mesh_path(env).await?;
    let file = store::read_file(path.clone()).await?;
    let stdout = if json {
        store::file_json(&file)?
    } else if status {
        store::status_text(&path, &file)?
    } else {
        store::list_text(&file)?
    };
    Ok(Report::success(stdout))
}

async fn forget(
    args: ForgetArgs,
    json: bool,
    env: &Environment,
    shutdown: &CancellationToken,
) -> Result<Report, Error> {
    let Some(id) = nonempty(args.target) else {
        return Ok(check::usage(FORGET_USAGE));
    };
    let local = env.node_name(&check::host_name()?)?;
    let mesh = store::mesh_path(env).await?;
    let file = store::read_file(mesh.clone()).await?;
    if !args.force && id == local {
        return Err(Error::ForgetLocal { name: local });
    }
    if !file.nodes.contains_key(&id) {
        return Err(Error::UnknownMeshNode { id });
    }
    store::forget(mesh.clone(), &id, shutdown).await?;
    let message = format!("Forgot {id}.\n");
    finish(Finished {
        action: EffectAction::Forgot,
        node: Some(id),
        state: None,
        mesh,
        message,
        warnings: Vec::new(),
        json,
    })
}

fn nonempty(value: Option<String>) -> Option<String> {
    match value {
        Some(value) if !value.is_empty() => Some(value),
        Some(_) | None => None,
    }
}

pub(super) fn finish(finished: Finished) -> Result<Report, Error> {
    let warnings = warning_text(&finished.warnings);
    let json = finished.json;
    let stdout = if json {
        format!("{}\n", serde_json::to_string(&effect(finished))?)
    } else {
        finished.message
    };
    Ok(Report {
        stdout,
        stderr: warnings,
        exit: Exit::Ok,
    })
}

fn effect(finished: Finished) -> Effect {
    Effect {
        action: finished.action,
        node: finished.node,
        state: finished.state.map(|path| path.display().to_string()),
        mesh: finished.mesh.display().to_string(),
        message: finished.message,
    }
}

fn warning_text(warnings: &[String]) -> String {
    let mut text = String::new();
    for warning in warnings {
        text.push_str(warning);
        text.push('\n');
    }
    text
}

fn settle(result: Result<Report, Error>) -> Result<Report, Error> {
    match result {
        Ok(report) => Ok(report),
        Err(error) if cancelled(&error) => Err(Error::Cancelled),
        Err(error) => Ok(hub(&error)),
    }
}

fn cancelled(error: &Error) -> bool {
    matches!(
        error,
        Error::Cancelled
            | Error::Pki(pki::Error::Cancelled)
            | Error::StepCa(stepca::Error::Cancelled)
            | Error::Registry(gateway::mesh::RegistryError::Cancelled)
    )
}

fn hub(error: &Error) -> Report {
    Report {
        stdout: String::new(),
        stderr: format!("  ❌ {error}\n"),
        exit: Exit::Failure,
    }
}
