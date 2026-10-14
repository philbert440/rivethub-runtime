use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use nix::errno::Errno;
use nix::fcntl::FcntlArg;
use nix::fcntl::OFlag;
use nix::sys::stat::Mode;
use thiserror::Error;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use wire::gateway::devices::AudioBackend;
use wire::gateway::devices::AudioFormat;
use wire::gateway::devices::AudioHealth;
use wire::gateway::devices::AudioStatus;

const DEVICE: &str = "RivetHub Mic";
const SAMPLE_RATE: u32 = 16_000;
pub(super) const CHANNELS: u32 = 1;

const FIFO_NAME: &str = "mic.pcm";
const FIFO_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;
const COMMANDS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioConfig {
    pub directory: PathBuf,
    pub device: String,
    pub sample_rate: u32,
}

impl AudioConfig {
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self {
            directory,
            device: DEVICE.to_owned(),
            sample_rate: SAMPLE_RATE,
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum AudioFault {
    #[error("another publisher is armed")]
    Busy,
    #[error("audio fifo has no reader")]
    NoReader,
    #[error("audio runtime is not ready")]
    NotReady,
    #[error("pcm length must be even")]
    Alignment,
    #[error("binary audio requires start")]
    NotArmed,
    #[error("audio bridge unavailable")]
    Unavailable,
    #[error("audio path escapes the audio directory")]
    Escape,
    #[error("audio directory is not a directory")]
    NotDirectory,
    #[error("audio path is not a fifo")]
    NotFifo,
    #[error("audio fifo path is not utf-8")]
    Path,
    #[error("audio byte counter overflowed")]
    Overflow,
    #[error("audio worker failed: {0}")]
    Worker(String),
    #[error("audio fifo failed: {0}")]
    Io(#[from] std::io::Error),
}

pub(crate) fn path_inside(root: &Path, candidate: &Path) -> bool {
    if root.as_os_str().is_empty() {
        return false;
    }
    let root = config::normalize(root);
    let candidate = config::normalize(candidate);
    if root.as_os_str().is_empty() || root == Path::new(".") {
        return false;
    }
    candidate.starts_with(&root)
}

fn path_text(path: &Path) -> Result<String, AudioFault> {
    match path.to_str() {
        Some(text) => Ok(text.to_owned()),
        None => Err(AudioFault::Path),
    }
}

fn prepare_fifo(directory: &Path) -> Result<PathBuf, AudioFault> {
    let directory = config::normalize(directory);
    if directory.as_os_str().is_empty() || directory == Path::new(".") {
        return Err(AudioFault::Escape);
    }
    match std::fs::symlink_metadata(&directory) {
        Ok(meta) if meta.file_type().is_symlink() => return Err(AudioFault::Escape),
        Ok(meta) if !meta.is_dir() => return Err(AudioFault::NotDirectory),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(&directory)?;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(DIR_MODE))?;
        }
        Err(error) => return Err(error.into()),
    }
    let fifo = directory.join(FIFO_NAME);
    if !path_inside(&directory, &fifo) {
        return Err(AudioFault::Escape);
    }
    ensure_fifo(&fifo)?;
    Ok(fifo)
}

fn ensure_fifo(fifo: &Path) -> Result<(), AudioFault> {
    match std::fs::symlink_metadata(fifo) {
        Ok(meta) if meta.file_type().is_symlink() => return Err(AudioFault::Escape),
        Ok(meta) if meta.file_type().is_fifo() => {
            std::fs::set_permissions(fifo, std::fs::Permissions::from_mode(FIFO_MODE))?;
            return Ok(());
        }
        Ok(_) => return Err(AudioFault::NotFifo),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    match nix::unistd::mkfifo(fifo, Mode::S_IRUSR | Mode::S_IWUSR) {
        Ok(()) => {}
        Err(Errno::EEXIST) => {
            let meta = std::fs::symlink_metadata(fifo)?;
            if meta.file_type().is_symlink() {
                return Err(AudioFault::Escape);
            }
            if !meta.file_type().is_fifo() {
                return Err(AudioFault::NotFifo);
            }
        }
        Err(error) => return Err(std::io::Error::other(error).into()),
    }
    std::fs::set_permissions(fifo, std::fs::Permissions::from_mode(FIFO_MODE))?;
    Ok(())
}

fn open_fifo(path: &Path) -> Result<std::fs::File, AudioFault> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(AudioFault::Escape);
    }
    if !meta.file_type().is_fifo() {
        return Err(AudioFault::NotFifo);
    }
    let fd = match nix::fcntl::open(
        path,
        OFlag::O_WRONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::ENXIO) => return Err(AudioFault::NoReader),
        Err(Errno::ELOOP) => return Err(AudioFault::Escape),
        Err(error) => return Err(std::io::Error::other(error).into()),
    };
    clear_nonblock(fd)
}

fn nix_fault(error: nix::Error) -> AudioFault {
    std::io::Error::other(error).into()
}

fn clear_nonblock(fd: std::os::fd::OwnedFd) -> Result<std::fs::File, AudioFault> {
    let bits = nix::fcntl::fcntl(&fd, FcntlArg::F_GETFL).map_err(nix_fault)?;
    let mut flags = OFlag::from_bits_truncate(bits);
    flags.remove(OFlag::O_NONBLOCK);
    nix::fcntl::fcntl(&fd, FcntlArg::F_SETFL(flags)).map_err(nix_fault)?;
    Ok(std::fs::File::from(fd))
}

struct Publisher {
    id: String,
    armed_at: DateTime<Utc>,
}

struct Runtime {
    config: AudioConfig,
    directory: PathBuf,
    fifo: PathBuf,
    ready: bool,
    fault: Option<String>,
    publisher: Option<Publisher>,
    writer: Option<tokio::fs::File>,
    bytes_written: u64,
}

impl Runtime {
    fn new(config: AudioConfig) -> Self {
        let directory = config::normalize(&config.directory);
        let fifo = directory.join(FIFO_NAME);
        Self {
            config,
            directory,
            fifo,
            ready: false,
            fault: None,
            publisher: None,
            writer: None,
            bytes_written: 0,
        }
    }

    async fn ensure(&mut self) -> Result<(), AudioFault> {
        if self.ready {
            return Ok(());
        }
        let directory = self.directory.clone();
        let prepared = match tokio::task::spawn_blocking(move || prepare_fifo(&directory)).await {
            Ok(result) => result,
            Err(error) => Err(AudioFault::Worker(error.to_string())),
        };
        match prepared {
            Ok(fifo) => {
                self.fifo = fifo;
                self.ready = true;
                self.fault = None;
                Ok(())
            }
            Err(fault) => {
                self.ready = false;
                let message = fault.to_string();
                tracing::warn!("audio runtime is not ready: {message}");
                self.fault = Some(message);
                Err(fault)
            }
        }
    }

    fn status(&self) -> Result<AudioStatus, AudioFault> {
        Ok(AudioStatus {
            enabled: true,
            backend: if self.ready {
                AudioBackend::FifoShim
            } else {
                AudioBackend::None
            },
            device: self.config.device.clone(),
            sample_rate: self.config.sample_rate,
            channels: CHANNELS,
            format: AudioFormat::S16Le,
            fifo_path: path_text(&self.fifo)?,
            armed: self.publisher.is_some(),
            publisher_id: self
                .publisher
                .as_ref()
                .map(|publisher| publisher.id.clone()),
            armed_at: self.publisher.as_ref().map(|publisher| publisher.armed_at),
            bytes_written: self.bytes_written,
            runtime_ready: self.ready,
        })
    }

    fn health(&self) -> Result<AudioHealth, AudioFault> {
        let message = if self.ready {
            None
        } else {
            Some(if let Some(text) = self.fault.clone() {
                text
            } else {
                AudioFault::NotReady.to_string()
            })
        };
        Ok(AudioHealth {
            ok: self.ready,
            runtime_ready: self.ready,
            fifo_path: path_text(&self.fifo)?,
            message,
        })
    }

    async fn report(&mut self) -> Result<AudioStatus, AudioFault> {
        if let Err(error) = self.ensure().await {
            tracing::debug!("audio ensure failed: {error}");
        }
        self.status()
    }

    async fn probe(&mut self) -> Result<AudioHealth, AudioFault> {
        if let Err(error) = self.ensure().await {
            tracing::debug!("audio ensure failed: {error}");
        }
        self.health()
    }

    async fn arm(&mut self) -> Result<(), AudioFault> {
        if self.publisher.is_some() {
            return Err(AudioFault::Busy);
        }
        self.ensure().await?;
        if !self.ready {
            return Err(AudioFault::NotReady);
        }
        let path = self.fifo.clone();
        let file = match tokio::task::spawn_blocking(move || open_fifo(&path)).await {
            Ok(result) => result?,
            Err(error) => return Err(AudioFault::Worker(error.to_string())),
        };
        self.writer = Some(tokio::fs::File::from_std(file));
        self.publisher = Some(Publisher {
            id: uuid::Uuid::new_v4().to_string(),
            armed_at: Utc::now(),
        });
        Ok(())
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<(), AudioFault> {
        if self.publisher.is_none() {
            return Err(AudioFault::NotArmed);
        }
        if !bytes.len().is_multiple_of(2) {
            return Err(AudioFault::Alignment);
        }
        let Some(writer) = self.writer.as_mut() else {
            return Err(AudioFault::NotArmed);
        };
        if let Err(error) = writer.write_all(bytes).await {
            self.disarm();
            return Err(error.into());
        }
        if let Err(error) = writer.flush().await {
            self.disarm();
            return Err(error.into());
        }
        let added = u64::try_from(bytes.len()).map_err(|_| AudioFault::Overflow)?;
        self.bytes_written = self
            .bytes_written
            .checked_add(added)
            .ok_or(AudioFault::Overflow)?;
        Ok(())
    }

    fn disarm(&mut self) {
        self.writer = None;
        self.publisher = None;
    }

    async fn dispatch(&mut self, command: Command) {
        match command {
            Command::Report(reply) => answer(reply, self.report().await),
            Command::Health(reply) => answer(reply, self.probe().await),
            Command::Arm(reply) => answer(reply, self.arm().await),
            Command::Write { bytes, reply } => answer(reply, self.write(&bytes).await),
            Command::Disarm(reply) => {
                self.disarm();
                answer(reply, Ok(()));
            }
        }
    }
}

fn answer<T>(reply: oneshot::Sender<T>, value: T) {
    if reply.send(value).is_err() {
        tracing::debug!("audio listener gone");
    }
}

enum Command {
    Report(oneshot::Sender<Result<AudioStatus, AudioFault>>),
    Health(oneshot::Sender<Result<AudioHealth, AudioFault>>),
    Arm(oneshot::Sender<Result<(), AudioFault>>),
    Write {
        bytes: Vec<u8>,
        reply: oneshot::Sender<Result<(), AudioFault>>,
    },
    Disarm(oneshot::Sender<Result<(), AudioFault>>),
}

async fn actor(
    mut commands: mpsc::Receiver<Command>,
    config: AudioConfig,
    shutdown: CancellationToken,
) {
    let mut runtime = Runtime::new(config);
    loop {
        let command = tokio::select! {
            () = shutdown.cancelled() => break,
            received = commands.recv() => match received {
                Some(command) => command,
                None => break,
            },
        };
        runtime.dispatch(command).await;
    }
}

#[derive(Clone)]
pub(crate) struct AudioCore {
    commands: mpsc::Sender<Command>,
    shutdown: CancellationToken,
}

impl AudioCore {
    pub(crate) async fn report(&self) -> Result<AudioStatus, AudioFault> {
        ask(&self.commands, Command::Report).await
    }

    pub(crate) async fn health(&self) -> Result<AudioHealth, AudioFault> {
        ask(&self.commands, Command::Health).await
    }

    pub(crate) async fn arm(&self) -> Result<(), AudioFault> {
        ask(&self.commands, Command::Arm).await
    }

    pub(crate) async fn disarm(&self) -> Result<(), AudioFault> {
        ask(&self.commands, Command::Disarm).await
    }

    pub(crate) async fn write(&self, bytes: Vec<u8>) -> Result<(), AudioFault> {
        let (tx, rx) = oneshot::channel();
        self.send(Command::Write { bytes, reply: tx }).await?;
        receive(rx).await
    }

    pub(crate) fn guard(&self) -> ArmGuard {
        ArmGuard {
            commands: self.commands.clone(),
            armed: false,
        }
    }

    pub(crate) fn shutdown(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    async fn send(&self, command: Command) -> Result<(), AudioFault> {
        match self.commands.send(command).await {
            Ok(()) => Ok(()),
            Err(error) => {
                tracing::debug!("audio command was not delivered: {error}");
                Err(AudioFault::Unavailable)
            }
        }
    }
}

async fn ask<T>(
    sender: &mpsc::Sender<Command>,
    command: impl FnOnce(oneshot::Sender<Result<T, AudioFault>>) -> Command,
) -> Result<T, AudioFault> {
    let (tx, rx) = oneshot::channel();
    match sender.send(command(tx)).await {
        Ok(()) => {}
        Err(error) => {
            tracing::debug!("audio command was not delivered: {error}");
            return Err(AudioFault::Unavailable);
        }
    }
    receive(rx).await
}

async fn receive<T>(reply: oneshot::Receiver<Result<T, AudioFault>>) -> Result<T, AudioFault> {
    match reply.await {
        Ok(result) => result,
        Err(error) => {
            tracing::debug!("audio reply was dropped: {error}");
            Err(AudioFault::Unavailable)
        }
    }
}

pub(crate) struct AudioBridge {
    core: AudioCore,
    tasks: TaskTracker,
    shutdown: CancellationToken,
}

impl AudioBridge {
    pub(crate) fn start(config: AudioConfig) -> Self {
        let (tx, rx) = mpsc::channel(COMMANDS);
        let shutdown = CancellationToken::new();
        let tasks = TaskTracker::new();
        tasks.spawn(actor(rx, config, shutdown.clone()));
        Self {
            core: AudioCore {
                commands: tx,
                shutdown: shutdown.clone(),
            },
            tasks,
            shutdown,
        }
    }

    pub(crate) fn core(&self) -> AudioCore {
        self.core.clone()
    }

    pub(crate) fn tasks(&self) -> TaskTracker {
        self.tasks.clone()
    }

    pub(crate) async fn shutdown(&self) {
        self.shutdown.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}

impl Drop for AudioBridge {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

pub(crate) struct ArmGuard {
    commands: mpsc::Sender<Command>,
    armed: bool,
}

impl ArmGuard {
    pub(crate) fn mark(&mut self) {
        self.armed = true;
    }

    pub(crate) fn clear(&mut self) {
        self.armed = false;
    }
}

impl Drop for ArmGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (reply, _) = oneshot::channel();
        if self.commands.try_send(Command::Disarm(reply)).is_err() {
            tracing::debug!("audio disarm on drop was not delivered");
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::path_inside;

    #[test]
    fn fifo_stays_inside_its_directory() {
        let root = Path::new("/tmp/rivethub-audio");
        assert!(path_inside(root, &root.join("mic.pcm")));
    }

    #[test]
    fn sibling_prefix_is_outside() {
        let root = Path::new("/tmp/rivethub-audio");
        assert!(!path_inside(
            root,
            Path::new("/tmp/rivethub-audio-evil/mic.pcm")
        ));
    }

    #[test]
    fn parent_segments_are_outside() {
        let root = Path::new("/tmp/rivethub-audio");
        assert!(!path_inside(
            root,
            Path::new("/tmp/rivethub-audio/../rivethub-audio-evil/mic.pcm")
        ));
    }

    #[test]
    fn empty_root_is_refused() {
        assert!(!path_inside(Path::new(""), Path::new("mic.pcm")));
    }
}
