use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use thiserror::Error;
use tokio::fs;
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

    pub async fn spool(&self, body: &str, now: DateTime<Utc>) -> Result<PathBuf, SpoolError> {
        fs::create_dir_all(&self.dir).await?;
        set_mode(&self.dir, DIR_MODE).await?;
        let name = format!(
            "{}-{}.json",
            now.timestamp_millis(),
            uuid::Uuid::now_v7().simple()
        );
        let target = self.dir.join(&name);
        let temporary = self.dir.join(format!("{name}.tmp"));
        fs::write(&temporary, body).await?;
        set_mode(&temporary, FILE_MODE).await?;
        fs::rename(&temporary, &target).await?;
        Ok(target)
    }

    pub async fn replay<F, Future_>(&self, max: usize, post: F) -> Result<(), ReplayError>
    where
        F: Fn(String) -> Future_,
        Future_: std::future::Future<Output = Result<CaptureResult, PostError>>,
    {
        let mut queued = queued_files(&self.dir).await?;
        queued.sort_by_key(|(prefix, _)| *prefix);
        for (_, path) in queued.into_iter().take(max) {
            let body = match fs::read_to_string(&path).await {
                Ok(body) => body,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(ReplayError::Io(error)),
            };
            match post(body).await {
                Ok(_) => match fs::remove_file(&path).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => return Err(ReplayError::Io(error)),
                },
                Err(PostError::Client { .. }) => self.dead_letter(&path).await?,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    async fn dead_letter(&self, path: &Path) -> Result<(), SpoolError> {
        let name = path.file_name().ok_or_else(|| {
            SpoolError::Io(std::io::Error::new(
                ErrorKind::InvalidData,
                "spool file has no name",
            ))
        })?;
        let dead = self.dir.join(DEAD_DIR);
        fs::create_dir_all(&dead).await?;
        fs::rename(path, dead.join(name)).await?;
        Ok(())
    }
}

async fn queued_files(dir: &Path) -> Result<Vec<(u64, PathBuf)>, std::io::Error> {
    let mut queued = Vec::new();
    let mut entries = match fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(queued),
        Err(error) => return Err(error),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let Some(prefix) = spool_prefix(&path) else {
            continue;
        };
        if fs::metadata(&path)
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            queued.push((prefix, path));
        }
    }
    Ok(queued)
}

fn spool_prefix(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let (digits, rest) = name.split_once('-')?;
    let is_spooled = Path::new(rest).extension().is_some_and(|ext| ext == "json");
    if !is_spooled || rest.contains('/') {
        return None;
    }
    digits.parse::<u64>().ok()
}

async fn set_mode(path: &Path, mode: u32) -> Result<(), std::io::Error> {
    fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).await
}
