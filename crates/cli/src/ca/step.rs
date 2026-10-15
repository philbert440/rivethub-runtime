use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;

use config::Environment;
use kernel::ca::Principal;
use strum::Display;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::TokenKind;
use super::paths::Material;
use super::roster;
use crate::{Error, Exit, Report};

const OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Display)]
#[strum(serialize_all = "snake_case")]
enum Provisioner {
    Nodes,
    Agents,
    Devices,
}

pub(super) struct Call<'a> {
    pub(super) material: &'a Material,
    pub(super) env: &'a Environment,
    pub(super) shutdown: &'a CancellationToken,
}

#[derive(Debug)]
struct Captured {
    stdout: String,
    stderr: String,
}

pub(super) async fn issue(
    kind: TokenKind,
    password: Option<PathBuf>,
    roster_path: Option<PathBuf>,
    call: &Call<'_>,
) -> Result<Report, Error> {
    let password = require_password(password).await?;
    let args = match kind {
        TokenKind::Node { name } => node_args(&name, roster_path, &password, call).await?,
        TokenKind::Agent { name } => agent_args(&name, roster_path, &password, call).await?,
        TokenKind::Device { name } => device_args(&name, &password, call)?,
    };
    token_report(spawn_step(&args, "token", call).await?)
}

pub(super) async fn revoke(
    serial: &str,
    password: Option<PathBuf>,
    call: &Call<'_>,
) -> Result<Report, Error> {
    canonical_serial(serial)?;
    let password = require_password(password).await?;
    let mut args = vec![
        OsString::from("ca"),
        OsString::from("revoke"),
        OsString::from(serial),
    ];
    push_endpoint(&mut args, &password, call);
    drop(spawn_step(&args, "revoke", call).await?);
    Ok(Report::success("Certificate revoked.\n"))
}

pub(super) fn connect(
    material: &Material,
    shutdown: &CancellationToken,
) -> Result<stepca::Authority, Error> {
    let config = stepca::Config::new(material.url.clone(), material.pem.clone())?;
    Ok(stepca::Authority::connect(config)?.with_cancellation(shutdown.clone()))
}

async fn node_args(
    name: &str,
    roster_path: Option<PathBuf>,
    password: &std::path::Path,
    call: &Call<'_>,
) -> Result<Vec<OsString>, Error> {
    let loaded = load_roster(roster_path).await?;
    let node = loaded.node(name)?;
    let mesh = mesh_name(&node.name);
    let sans = [node.address.to_string(), node.name.clone(), mesh];
    Ok(token_args(
        &node.name,
        &sans,
        Provisioner::Nodes,
        password,
        call,
    ))
}

async fn agent_args(
    name: &str,
    roster_path: Option<PathBuf>,
    password: &std::path::Path,
    call: &Call<'_>,
) -> Result<Vec<OsString>, Error> {
    let loaded = load_roster(roster_path).await?;
    let (node, agent) = loaded.agent(name)?;
    let uri = Principal::Agent {
        node: node.name.clone(),
        agent: agent.to_owned(),
    }
    .spiffe_uri()?;
    Ok(token_args(
        &uri,
        std::slice::from_ref(&uri),
        Provisioner::Agents,
        password,
        call,
    ))
}

fn device_args(
    name: &str,
    password: &std::path::Path,
    call: &Call<'_>,
) -> Result<Vec<OsString>, Error> {
    let principal = Principal::Node {
        node: name.to_owned(),
    };
    if principal.spiffe_uri().is_err() {
        return Err(Error::DeviceName);
    }
    let owned = name.to_owned();
    Ok(token_args(
        &owned,
        std::slice::from_ref(&owned),
        Provisioner::Devices,
        password,
        call,
    ))
}

fn token_args(
    subject: &str,
    sans: &[String],
    provisioner: Provisioner,
    password: &std::path::Path,
    call: &Call<'_>,
) -> Vec<OsString> {
    let mut args = vec![
        OsString::from("ca"),
        OsString::from("token"),
        OsString::from(subject),
        OsString::from("--provisioner"),
        OsString::from(provisioner.to_string()),
    ];
    for san in sans {
        args.push(OsString::from("--san"));
        args.push(OsString::from(san));
    }
    push_endpoint(&mut args, password, call);
    args
}

fn push_endpoint(args: &mut Vec<OsString>, password: &std::path::Path, call: &Call<'_>) {
    args.push(OsString::from("--ca-url"));
    args.push(OsString::from(call.material.url.as_str()));
    args.push(OsString::from("--root"));
    args.push(call.material.root.as_os_str().to_owned());
    args.push(OsString::from("--provisioner-password-file"));
    args.push(password.as_os_str().to_owned());
}

async fn load_roster(path: Option<PathBuf>) -> Result<roster::Roster, Error> {
    let path = path.ok_or(Error::RosterRequired)?;
    let text = tokio::task::spawn_blocking(move || super::paths::roster_text(&path)).await??;
    roster::parse(&text)
}

async fn require_password(path: Option<PathBuf>) -> Result<PathBuf, Error> {
    let path = path.ok_or(Error::PasswordRequired)?;
    tokio::task::spawn_blocking(move || super::paths::password_file(&path)).await?
}

fn mesh_name(name: &str) -> String {
    format!("{name}.rivet.mesh")
}

fn canonical_serial(serial: &str) -> Result<(), Error> {
    let mut bytes = serial.bytes();
    match bytes.next() {
        Some(first)
            if (b'1'..=b'9').contains(&first) && bytes.all(|byte| byte.is_ascii_digit()) =>
        {
            Ok(())
        }
        _ => Err(Error::Serial),
    }
}

fn token_report(captured: Captured) -> Result<Report, Error> {
    let token = one_time_token(&captured.stdout)?;
    Ok(Report {
        stdout: format!("{token}\n"),
        stderr: captured.stderr,
        exit: Exit::Ok,
    })
}

fn one_time_token(stdout: &str) -> Result<&str, Error> {
    let token = stdout.trim_end_matches(['\r', '\n']);
    if token.is_empty() || token.bytes().any(|byte| byte.is_ascii_whitespace()) {
        Err(Error::StepToken)
    } else {
        Ok(token)
    }
}

fn command(env: &Environment) -> Command {
    let mut command = Command::new("step");
    command.env_clear().envs(env.iter()).kill_on_drop(true);
    command
}

async fn spawn_step(
    args: &[OsString],
    action: &'static str,
    call: &Call<'_>,
) -> Result<Captured, Error> {
    if call.shutdown.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let mut command = command(call.env);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let stdout = child.stdout.take().ok_or(Error::StepPipe)?;
    let stderr = child.stderr.take().ok_or(Error::StepPipe)?;
    let stdout_task = tokio::spawn(read_pipe(stdout));
    let stderr_task = tokio::spawn(read_pipe(stderr));
    tokio::select! {
        biased;
        () = call.shutdown.cancelled() => {
            let killed = child.kill().await;
            stdout_task.abort();
            stderr_task.abort();
            killed?;
            Err(Error::Cancelled)
        }
        status = child.wait() => {
            let status = status?;
            let stdout = text(stdout_task.await??)?;
            let stderr = text(stderr_task.await??)?;
            if status.success() {
                Ok(Captured { stdout, stderr })
            } else {
                Err(Error::StepFailed { action, detail: stderr })
            }
        }
    }
}

async fn read_pipe(mut pipe: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = pipe.read(&mut chunk).await?;
        if count == 0 {
            return Ok(bytes);
        }
        let next = bytes.len().saturating_add(count);
        if next > OUTPUT_LIMIT {
            return Err(Error::StepOutput);
        }
        bytes.extend(chunk.iter().take(count).copied());
    }
}

fn text(bytes: Vec<u8>) -> Result<String, Error> {
    String::from_utf8(bytes).map_err(|_| Error::StepEncoding)
}
