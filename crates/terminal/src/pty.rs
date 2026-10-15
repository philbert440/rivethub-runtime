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

const READ_BUFFER: usize = 8192;
pub(crate) const MISSING_BINARY_OUTPUT: &[u8] = b"execvp(3) failed.: No such file or directory\r\n";
pub(crate) const MISSING_BINARY_EXIT: u32 = 1;

pub(crate) struct Spawned {
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
pub(crate) struct ExitOutcome {
    pub code: u32,
}

#[derive(Debug)]
pub(crate) enum SpawnOutcome {
    Live(Spawned),
    MissingBinary,
}

#[must_use]
fn recorded_exit_code(status: &portable_pty::ExitStatus) -> u32 {
    if status.signal().is_some() {
        0
    } else {
        status.exit_code()
    }
}

pub(crate) fn spawn(
    argv: &[String],
    cwd: &Path,
    size: kernel::terminal::TermSize,
    env: &BTreeMap<String, OsString>,
) -> Result<SpawnOutcome, Error> {
    if argv.is_empty() {
        return Err(Error::EmptyArgv);
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
        .openpty(PtySize {
            cols: size.cols,
            rows: size.rows,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| Error::PtyBackend(error.into()))?;
    match pair.slave.spawn_command(command) {
        Ok(child) => Ok(SpawnOutcome::Live(Spawned {
            pid: child.process_id(),
            child,
            master: pair.master,
        })),
        Err(error) if is_missing_binary(error.chain()) => Ok(SpawnOutcome::MissingBinary),
        Err(error) => Err(Error::PtyBackend(error.into())),
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
    if pid == 0 {
        return Err(Error::InvalidPid(pid));
    }
    let target = i32::try_from(pid).map_err(|_| Error::InvalidPid(pid))?;
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(target), signal).map_err(|source| {
        Error::Signal {
            pid,
            signal,
            source,
        }
    })
}

pub(crate) fn spawn_exit_watcher(
    tasks: &tokio_util::task::TaskTracker,
    mut child: Box<dyn Child + Send + Sync>,
    outcome: tokio::sync::oneshot::Sender<Result<ExitOutcome, Error>>,
) {
    tasks.spawn_blocking(move || {
        let result = child
            .wait()
            .map(|status| ExitOutcome {
                code: recorded_exit_code(&status),
            })
            .map_err(Error::Io);
        drop(outcome.send(result));
    });
}

pub(crate) fn spawn_reader(
    tasks: &tokio_util::task::TaskTracker,
    mut reader: Box<dyn Read + Send>,
    sink: tokio::sync::mpsc::Sender<Result<Vec<u8>, Error>>,
) {
    tasks.spawn_blocking(move || {
        loop {
            let mut chunk = vec![0u8; READ_BUFFER];
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => {
                    drop(sink.blocking_send(Err(Error::Io(error))));
                    break;
                }
                Ok(read) => {
                    chunk.truncate(read);
                    if sink.blocking_send(Ok(chunk)).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

#[derive(Debug)]
pub(crate) struct WriteRequest {
    pub(crate) bytes: Vec<u8>,
    pub(crate) result: tokio::sync::oneshot::Sender<Result<(), Error>>,
}

pub(crate) fn spawn_writer(
    tasks: &tokio_util::task::TaskTracker,
    mut writer: Box<dyn Write + Send>,
    mut source: tokio::sync::mpsc::Receiver<WriteRequest>,
) {
    tasks.spawn_blocking(move || {
        while let Some(request) = source.blocking_recv() {
            let result = writer.write_all(&request.bytes).map_err(Error::Io);
            let failed = result.is_err();
            drop(request.result.send(result));
            if failed {
                break;
            }
        }
    });
}
