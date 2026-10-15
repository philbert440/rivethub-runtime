use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use kernel::harness::SessionEvent;
use tokio::io::AsyncBufReadExt as _;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::AcpError;
use crate::launch::AcpCommand;

pub(crate) struct ChildPipes {
    pub(crate) child: tokio::process::Child,
    pub(crate) stdin: tokio::process::ChildStdin,
    pub(crate) stdout: tokio::process::ChildStdout,
    pub(crate) stderr: tokio::process::ChildStderr,
}

pub(crate) fn spawn_child(
    command: &AcpCommand,
    working_dir: &Path,
) -> Result<ChildPipes, AcpError> {
    let program = command.program.display().to_string();
    let mut child = tokio::process::Command::new(&command.program)
        .args(&command.args)
        .current_dir(working_dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| AcpError::Launch {
            program: program.clone(),
            source,
        })?;
    let stdin = child.stdin.take().ok_or_else(|| AcpError::Stdio {
        program: program.clone(),
    })?;
    let stdout = child.stdout.take().ok_or_else(|| AcpError::Stdio {
        program: program.clone(),
    })?;
    let stderr = child.stderr.take().ok_or(AcpError::Stdio { program })?;
    Ok(ChildPipes {
        child,
        stdin,
        stdout,
        stderr,
    })
}

pub(crate) fn watch_child(
    mut child: tokio::process::Child,
    exited: Arc<AtomicBool>,
    events: mpsc::Sender<SessionEvent>,
    shutdown: CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) -> tokio::sync::oneshot::Receiver<Result<(), std::io::Error>> {
    let (done, reaped) = tokio::sync::oneshot::channel();
    tasks.spawn(async move {
        let outcome = tokio::select! {
            () = shutdown.cancelled() => child.kill().await,
            status = child.wait() => match status {
                Ok(status) => {
                let code = status.code();
                exited.store(true, Ordering::Release);
                tokio::select! {
                    () = shutdown.cancelled() => tracing::trace!("agent exited during release"),
                    result = events.send(SessionEvent::Exited { code }) => {
                        if result.is_err() {
                            tracing::debug!("session event stream closed before the agent exit");
                        }
                    }
                }
                Ok(())
                }
                Err(error) => Err(error),
            },
        };
        if done.send(outcome).is_err() {
            tracing::trace!("harness child reaped without a release waiter");
        }
    });
    reaped
}

pub(crate) async fn pump_stderr(stderr: tokio::process::ChildStderr) -> std::io::Result<()> {
    let mut lines = tokio::io::BufReader::new(stderr).lines();
    while let Some(line) = lines.next_line().await? {
        tracing::debug!(target: "rivethub::harness_acp::agent", "{line}");
    }
    Ok(())
}

pub(crate) fn watch_stderr(
    stderr: tokio::process::ChildStderr,
    shutdown: CancellationToken,
    tasks: &tokio_util::task::TaskTracker,
) {
    tasks.spawn(async move {
        tokio::select! {
            () = shutdown.cancelled() => {},
            result = pump_stderr(stderr) => {
                if let Err(error) = result {
                    tracing::warn!(%error, "agent stderr failed");
                }
            }
        }
    });
}
