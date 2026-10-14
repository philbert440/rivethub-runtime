use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Read;
use std::io::Write;
use std::path::Path;

use portable_pty::Child;
use portable_pty::CommandBuilder;
use portable_pty::MasterPty;
use portable_pty::PtySize;
use portable_pty::native_pty_system;

use crate::Error;

pub const READ_BUFFER: usize = 8192;
pub const MISSING_BINARY_OUTPUT: &[u8] = b"execvp(3) failed.: No such file or directory\r\n";
pub const MISSING_BINARY_EXIT: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl From<kernel::terminal::TermSize> for Size {
    fn from(value: kernel::terminal::TermSize) -> Self {
        Self {
            cols: value.cols,
            rows: value.rows,
        }
    }
}

impl From<Size> for PtySize {
    fn from(value: Size) -> Self {
        Self {
            cols: value.cols,
            rows: value.rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }
}

pub struct Spawned {
    pub master: Box<dyn MasterPty + Send>,
    pub pid: Option<u32>,
    pub child: Box<dyn Child + Send + Sync>,
}

#[cfg_attr(
    dylint_lib = "rr_lints",
    expect(
        manualderive,
        reason = "the master and child handles are opaque crates types without Debug, so the derive does not compile; the impl prints only the pid"
    )
)]
impl std::fmt::Debug for Spawned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spawned")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitOutcome {
    pub code: u32,
}

#[derive(Debug)]
pub enum SpawnOutcome {
    Live(Spawned),
    MissingBinary,
}

#[must_use]
pub fn recorded_exit_code(status: &portable_pty::ExitStatus) -> u32 {
    if status.signal().is_some() {
        0
    } else {
        status.exit_code()
    }
}

pub fn spawn(
    argv: &[String],
    cwd: &Path,
    size: Size,
    env: &BTreeMap<String, OsString>,
) -> Result<SpawnOutcome, Error> {
    if argv.is_empty() {
        return Err(Error::Spawn("argv is empty".to_owned()));
    }
    let mut command = CommandBuilder::from_argv(argv.iter().map(OsString::from).collect());
    command.cwd(cwd);
    for (name, value) in env {
        command.env(name, value);
    }
    command.env_remove("RIVETHUB_USER_ID");
    command.env_remove("RIVETHUB_USER_TOKEN");
    command.env_remove("RIVETOS_USER_ID");
    command.env_remove("RIVETOS_USER_TOKEN");
    let system = native_pty_system();
    let pair = system
        .openpty(size.into())
        .map_err(|error| Error::Spawn(error.to_string()))?;
    match pair.slave.spawn_command(command) {
        Ok(child) => Ok(SpawnOutcome::Live(Spawned {
            pid: child.process_id(),
            child,
            master: pair.master,
        })),
        Err(error) if is_missing_binary(error.chain()) => Ok(SpawnOutcome::MissingBinary),
        Err(error) => Err(Error::Spawn(error.to_string())),
    }
}

fn is_missing_binary<'a>(
    chain: impl Iterator<Item = &'a (dyn std::error::Error + 'static)>,
) -> bool {
    for cause in chain {
        if cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io_error| io_error.kind() == std::io::ErrorKind::NotFound)
        {
            return true;
        }
        if cause
            .to_string()
            .contains("No viable candidates found in PATH")
        {
            return true;
        }
    }
    false
}

pub fn signal(pid: u32, signal: nix::sys::signal::Signal) -> Result<(), Error> {
    let Ok(target) = i32::try_from(pid) else {
        return Err(Error::Spawn(format!(
            "pid {pid} does not fit a signal target"
        )));
    };
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(target), signal)
        .map_err(|error| Error::Spawn(format!("{signal} to {pid} failed: {error}")))
}

pub fn spawn_exit_watcher(
    mut child: Box<dyn Child + Send + Sync>,
    outcome: tokio::sync::oneshot::Sender<ExitOutcome>,
) {
    std::thread::spawn(move || {
        let code = match child.wait() {
            Ok(status) => recorded_exit_code(&status),
            Err(error) => {
                tracing::warn!("terminal exit watch failed ({error}) — recorded as exit 0");
                0
            }
        };
        #[expect(
            clippy::let_underscore_must_use,
            reason = "the exit watcher sends once; the receiver lives in the manager and outlives this thread"
        )]
        let _: Result<(), ExitOutcome> = outcome.send(ExitOutcome { code });
    });
}

pub fn spawn_reader(mut reader: Box<dyn Read + Send>, sink: tokio::sync::mpsc::Sender<Vec<u8>>) {
    std::thread::spawn(move || {
        loop {
            let mut chunk = vec![0u8; READ_BUFFER];
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    chunk.truncate(read);
                    if sink.blocking_send(chunk).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

pub fn spawn_writer(
    mut writer: Box<dyn Write + Send>,
    mut source: tokio::sync::mpsc::Receiver<Vec<u8>>,
) {
    std::thread::spawn(move || {
        while let Some(bytes) = source.blocking_recv() {
            if writer.write_all(&bytes).is_err() {
                break;
            }
        }
    });
}
