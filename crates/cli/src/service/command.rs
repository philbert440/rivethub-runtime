use std::os::unix::process::ExitStatusExt;
use std::process::{ExitStatus, Stdio};

use config::Environment;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::ServiceError;
use crate::{Exit, Report};

pub(super) fn command(program: &'static str, env: &Environment) -> Command {
    let mut command = Command::new(program);
    command.env_clear().envs(env.iter()).kill_on_drop(true);
    command
}

pub(super) async fn capture(
    mut command: Command,
    program: &'static str,
    shutdown: &CancellationToken,
) -> Result<Report, ServiceError> {
    if shutdown.is_cancelled() {
        return Err(ServiceError::Cancelled);
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|source| ServiceError::Process { program, source })?;
    let stdout = child.stdout.take().ok_or(ServiceError::Pipe)?;
    let stderr = child.stderr.take().ok_or(ServiceError::Pipe)?;
    let output = async {
        let read = async |mut pipe: tokio::process::ChildStdout| {
            use tokio::io::AsyncReadExt;
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).await?;
            Ok::<_, std::io::Error>(bytes)
        };
        let errors = async {
            use tokio::io::AsyncReadExt;
            let mut pipe = stderr;
            let mut bytes = Vec::new();
            pipe.read_to_end(&mut bytes).await?;
            Ok::<_, std::io::Error>(bytes)
        };
        tokio::try_join!(read(stdout), errors, child.wait())
    };
    tokio::select! {
        biased;
        () = shutdown.cancelled() => {
            child.kill().await.map_err(|source| ServiceError::Process { program, source })?;
            Err(ServiceError::Cancelled)
        }
        result = output => {
            let (stdout, stderr, status) = result.map_err(|source| ServiceError::Process { program, source })?;
            Ok(Report {
                stdout: String::from_utf8(stdout)?,
                stderr: String::from_utf8(stderr)?,
                exit: exit(status)?,
            })
        }
    }
}

pub(super) fn require_success(report: Report) -> Result<Report, ServiceError> {
    if report.exit == Exit::Ok {
        Ok(report)
    } else {
        Err(ServiceError::Failed {
            exit: report.exit,
            stderr: report.stderr,
        })
    }
}

pub(super) async fn follow(
    mut command: Command,
    shutdown: &CancellationToken,
) -> Result<Report, ServiceError> {
    if shutdown.is_cancelled() {
        return Ok(Report::success(""));
    }
    let program = "journalctl";
    let mut child = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|source| ServiceError::Process { program, source })?;
    let status = tokio::select! {
        biased;
        () = shutdown.cancelled() => {
            child.kill().await.map_err(|source| ServiceError::Process { program, source })?;
            return Ok(Report::success(""));
        }
        result = child.wait() => result.map_err(|source| ServiceError::Process { program, source })?,
    };
    let exit = if status.signal() == Some(nix::sys::signal::Signal::SIGINT as i32) {
        Exit::Ok
    } else {
        exit(status)?
    };
    Ok(Report {
        stdout: String::new(),
        stderr: String::new(),
        exit,
    })
}

fn exit(status: ExitStatus) -> Result<Exit, ServiceError> {
    match status.code() {
        Some(0) => Ok(Exit::Ok),
        Some(1) => Ok(Exit::Failure),
        Some(code) => Ok(Exit::Child(
            u8::try_from(code).map_err(|_| ServiceError::Exit(status))?,
        )),
        None => Err(ServiceError::Exit(status)),
    }
}
