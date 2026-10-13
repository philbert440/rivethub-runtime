use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use thiserror::Error;
use wire::capture::CaptureResult;

use crate::writer::PostError;

const FILE_MODE: u32 = 0o600;
const DIR_MODE: u32 = 0o700;
const DEAD_DIR: &str = "dead";

#[derive(Debug, Error)]
pub enum SpoolError {
    #[error("capture spool failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("capture spool replay failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Spool(#[from] SpoolError),
    #[error(transparent)]
    Post(#[from] PostError),
}

#[derive(Debug)]
pub struct CaptureSpool {
    dir: PathBuf,
}

impl CaptureSpool {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn spool(&self, body: &str, now: DateTime<Utc>) -> Result<PathBuf, SpoolError> {
        fs::create_dir_all(&self.dir)?;
        set_mode(&self.dir, DIR_MODE)?;
        let name = format!(
            "{}-{}.json",
            now.timestamp_millis(),
            uuid::Uuid::now_v7().simple()
        );
        let target = self.dir.join(&name);
        let temporary = self.dir.join(format!("{name}.tmp"));
        fs::write(&temporary, body)?;
        set_mode(&temporary, FILE_MODE)?;
        fs::rename(&temporary, &target)?;
        Ok(target)
    }

    pub async fn replay<F, Future_>(&self, max: usize, post: F) -> Result<(), ReplayError>
    where
        F: Fn(String) -> Future_,
        Future_: std::future::Future<Output = Result<CaptureResult, PostError>>,
    {
        let mut queued = queued_files(&self.dir)?;
        queued.sort_by_key(|(prefix, _)| *prefix);
        for (_, path) in queued.into_iter().take(max) {
            let body = match fs::read_to_string(&path) {
                Ok(body) => body,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(ReplayError::Io(error)),
            };
            match post(body).await {
                Ok(_) => {
                    let _ = fs::remove_file(&path);
                }
                Err(PostError::Client { .. }) => {
                    self.dead_letter(&path)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn dead_letter(&self, path: &Path) -> Result<(), SpoolError> {
        let name = path.file_name().ok_or_else(|| {
            SpoolError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "spool file has no name",
            ))
        })?;
        let dead = self.dir.join(DEAD_DIR);
        fs::create_dir_all(&dead)?;
        fs::rename(path, dead.join(name.to_string_lossy().to_string()))?;
        Ok(())
    }
}

fn queued_files(dir: &Path) -> Result<Vec<(u64, PathBuf)>, std::io::Error> {
    let mut queued = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(queued),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        if let Some(prefix) = spool_prefix(&path) {
            queued.push((prefix, path));
        }
    }
    Ok(queued)
}

fn spool_prefix(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_string_lossy();
    let (digits, rest) = name.split_once('-')?;
    let is_spooled = std::path::Path::new(rest).extension() == Some(std::ffi::OsStr::new("json"));
    if !is_spooled || rest.contains('/') {
        return None;
    }
    digits.parse::<u64>().ok()
}

fn set_mode(path: &Path, mode: u32) -> Result<(), std::io::Error> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)
}
