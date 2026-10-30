use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use tokio::process::{Child, Command};
use wire::plugin::PluginId;
use wire::plugin::manifest::{Permission, PluginRun};

use super::sandbox::{self, Confinement};

const STOP_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub(super) struct LaunchPlan {
    pub(super) program: PathBuf,
    pub(super) args: Vec<String>,
    pub(super) directory: PathBuf,
    pub(super) env: Vec<(OsString, OsString)>,
    pub(super) sandbox: Option<sandbox::Request>,
}

#[derive(Debug)]
pub(super) struct Contract<'a> {
    pub(super) plugin: &'a PluginId,
    pub(super) node: &'a str,
    pub(super) data: &'a Path,
    pub(super) run: &'a Path,
    pub(super) socket: &'a Path,
    pub(super) api: &'a Path,
    pub(super) token: &'a str,
    pub(super) confine: Option<&'a [Permission]>,
}

#[derive(Debug)]
pub(super) struct Launched {
    pub(super) child: Child,
    pub(super) confinement: Confinement,
}

pub(super) fn plan(
    run: &PluginRun,
    directory: &Path,
    inherited: &[(OsString, OsString)],
    contract: &Contract<'_>,
) -> Option<LaunchPlan> {
    let (first, args) = run.command.split_first()?;
    let program = if first.contains('/') {
        directory.join(first)
    } else {
        PathBuf::from(first)
    };
    let mut env: Vec<(OsString, OsString)> = inherited.to_vec();
    env.extend(
        run.env
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value))),
    );
    env.extend([
        pair("RIVETHUB_PLUGIN_ID", contract.plugin.as_ref()),
        pair("RIVETHUB_PLUGIN_NODE", contract.node),
        (
            OsString::from("RIVETHUB_PLUGIN_DATA"),
            contract.data.as_os_str().to_owned(),
        ),
        (
            OsString::from("RIVETHUB_PLUGIN_SOCKET"),
            contract.socket.as_os_str().to_owned(),
        ),
        (
            OsString::from("RIVETHUB_API_SOCKET"),
            contract.api.as_os_str().to_owned(),
        ),
        pair("RIVETHUB_PLUGIN_TOKEN", contract.token),
        (
            OsString::from("TMPDIR"),
            contract.run.as_os_str().to_owned(),
        ),
    ]);
    let search = env
        .iter()
        .rev()
        .find(|(name, _)| name == "PATH")
        .map(|(_, value)| value.clone());
    let sandbox = contract.confine.map(|permissions| sandbox::Request {
        permissions: permissions.to_vec(),
        folders: sandbox::Folders {
            install: directory.to_path_buf(),
            data: contract.data.to_path_buf(),
            run: contract.run.to_path_buf(),
            api: contract.api.to_path_buf(),
        },
        program: program.clone(),
        search,
    });
    Some(LaunchPlan {
        program,
        args: args.to_vec(),
        directory: directory.to_path_buf(),
        env,
        sandbox,
    })
}

fn pair(name: &str, value: &str) -> (OsString, OsString) {
    (OsString::from(name), OsString::from(value))
}

pub(super) async fn spawn(plan: &LaunchPlan) -> Result<Launched, String> {
    let mut command = Command::new(&plan.program);
    command
        .args(&plan.args)
        .current_dir(&plan.directory)
        .env_clear()
        .envs(plan.env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let Some(request) = plan.sandbox.clone() else {
        return command
            .spawn()
            .map(|child| Launched {
                child,
                confinement: Confinement::Unconfined,
            })
            .map_err(|error| error.to_string());
    };
    let runtime = tokio::runtime::Handle::current();
    let (sent, launched) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("plugin-launch".to_owned())
        .spawn(move || {
            let _entered = runtime.enter();
            let started = sandbox::resolve(&request)
                .and_then(|policy| {
                    sandbox::restrict(&policy)
                        .map_err(|error| format!("confinement failed: {error}"))
                })
                .and_then(|confinement| {
                    command
                        .spawn()
                        .map(|child| Launched { child, confinement })
                        .map_err(|error| error.to_string())
                });
            if sent.send(started).is_err() {
                tracing::debug!("plug-in launch abandoned; its process is stopped");
            }
        })
        .map_err(|error| format!("launch thread: {error}"))?;
    launched
        .await
        .map_err(|error| format!("launch thread: {error}"))?
}

pub(super) async fn terminate(child: &mut Child) -> std::io::Result<std::process::ExitStatus> {
    let group = child.id().map(|pid| Pid::from_raw(pid.cast_signed()));
    if let Some(group) = group {
        signal_group(group, Signal::SIGTERM);
    }
    match tokio::time::timeout(STOP_GRACE, child.wait()).await {
        Ok(status) => status,
        Err(elapsed) => {
            tracing::debug!(%elapsed, "plug-in ignored SIGTERM; killing its process group");
            if let Some(group) = group {
                signal_group(group, Signal::SIGKILL);
            }
            child.wait().await
        }
    }
}

fn signal_group(group: Pid, signal: Signal) {
    if let Err(error) = killpg(group, signal) {
        tracing::debug!(%error, "plug-in process group already gone");
    }
}
