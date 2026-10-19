use std::fs::{self, File};
use std::io::{ErrorKind, Read as _};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, SubsecRound as _, Utc};
use serde::Deserialize;
use tempfile::NamedTempFile;
use tokio_util::sync::CancellationToken;
use wire::mesh::{MeshFile, MeshNode};

use super::registry::{RegistryError, validate};

const ABANDONED_LOCK: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(super) struct Storage {
    root: PathBuf,
    path: PathBuf,
    lock: PathBuf,
}

impl Storage {
    pub(super) fn open(root: &Path, relative: &Path) -> Result<Self, RegistryError> {
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| part == Component::ParentDir)
        {
            return Err(RegistryError::OutsideRoot(relative.to_path_buf()));
        }
        let root = root.canonicalize()?;
        let candidate = root.join(relative);
        let path = match candidate.canonicalize() {
            Ok(path) if path.starts_with(&root) => path,
            Ok(_) => return Err(RegistryError::OutsideRoot(candidate)),
            Err(error) if error.kind() == ErrorKind::NotFound => candidate,
            Err(error) => return Err(error.into()),
        };
        let mut lock = path.as_os_str().to_os_string();
        lock.push(".lock");
        let storage = Self {
            root,
            path,
            lock: PathBuf::from(lock),
        };
        storage.check_paths()?;
        Ok(storage)
    }

    fn check_paths(&self) -> Result<(), RegistryError> {
        for path in [&self.path, &self.lock] {
            let parent = path
                .parent()
                .ok_or_else(|| RegistryError::OutsideRoot(path.clone()))?;
            if !parent.canonicalize()?.starts_with(&self.root) {
                return Err(RegistryError::OutsideRoot(path.clone()));
            }
            match path.canonicalize() {
                Ok(resolved) if !resolved.starts_with(&self.root) => {
                    return Err(RegistryError::OutsideRoot(path.clone()));
                }
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    match fs::symlink_metadata(path) {
                        Ok(metadata) if metadata.file_type().is_symlink() => {
                            return Err(RegistryError::OutsideRoot(path.clone()));
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == ErrorKind::NotFound => {}
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    pub(super) fn read(&self) -> Result<Option<MeshFile>, RegistryError> {
        let Some(bytes) = self.bytes()? else {
            return Ok(None);
        };
        Ok(Some(parse(&bytes)?))
    }

    fn bytes(&self) -> Result<Option<Vec<u8>>, RegistryError> {
        self.check_paths()?;
        match File::open(&self.path) {
            Ok(mut file) => {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                Ok(Some(bytes))
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn change(
        &self,
        now: DateTime<Utc>,
        shutdown: &CancellationToken,
        edit: impl FnOnce(&mut MeshFile) -> Result<(), RegistryError>,
    ) -> Result<MeshFile, RegistryError> {
        self.check_paths()?;
        let now = now.trunc_subsecs(3);
        check_cancelled(shutdown)?;
        match fs::create_dir(&self.lock) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => self.take_abandoned_lock()?,
            Err(error) => return Err(error.into()),
        }
        let result = self.commit(now, shutdown, edit);
        fs::remove_dir(&self.lock)?;
        result
    }

    fn take_abandoned_lock(&self) -> Result<(), RegistryError> {
        let modified = match fs::metadata(&self.lock) {
            Ok(metadata) => metadata.modified()?,
            Err(error) if error.kind() == ErrorKind::NotFound => return Err(RegistryError::Busy),
            Err(error) => return Err(error.into()),
        };
        let abandoned = modified
            .checked_add(ABANDONED_LOCK)
            .is_some_and(|deadline| deadline <= SystemTime::now());
        if !abandoned {
            return Err(RegistryError::Busy);
        }
        tracing::warn!(lock = %self.lock.display(), "removing an abandoned mesh registry lock");
        match fs::remove_dir(&self.lock) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        match fs::create_dir(&self.lock) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(RegistryError::Busy),
            Err(error) => Err(error.into()),
        }
    }

    fn commit(
        &self,
        now: DateTime<Utc>,
        shutdown: &CancellationToken,
        edit: impl FnOnce(&mut MeshFile) -> Result<(), RegistryError>,
    ) -> Result<MeshFile, RegistryError> {
        let original = self.bytes()?;
        let mut file = match &original {
            Some(bytes) => parse(bytes)?,
            None => MeshFile::empty(now),
        };
        edit(&mut file)?;
        validate(&file)?;
        file.updated_at = now.max(file.updated_at);
        let parent = self
            .path
            .parent()
            .ok_or_else(|| RegistryError::OutsideRoot(self.path.clone()))?;
        let mut temporary = NamedTempFile::new_in(parent)?;
        match fs::metadata(&self.path) {
            Ok(metadata) => temporary
                .as_file()
                .set_permissions(metadata.permissions())?,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt as _;
                    temporary
                        .as_file()
                        .set_permissions(fs::Permissions::from_mode(0o644))?;
                }
            }
            Err(error) => return Err(error.into()),
        }
        serde_json::to_writer_pretty(&mut temporary, &file)?;
        temporary.as_file().sync_all()?;
        check_cancelled(shutdown)?;
        if self.bytes()? != original {
            return Err(RegistryError::Conflict);
        }
        temporary
            .persist(&self.path)
            .map_err(|error| RegistryError::Io(error.error))?;
        File::open(parent)?.sync_all()?;
        Ok(file)
    }
}

fn parse(bytes: &[u8]) -> Result<MeshFile, RegistryError> {
    let document: StoredRegistry = serde_json::from_slice(bytes)?;
    let file = match document {
        StoredRegistry::Record(file) => file,
        StoredRegistry::Legacy { nodes } => {
            drop(nodes);
            return Err(RegistryError::FlatArray);
        }
    };
    validate(&file)?;
    Ok(file)
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredRegistry {
    Record(MeshFile),
    Legacy { nodes: Vec<MeshNode> },
}

fn check_cancelled(shutdown: &CancellationToken) -> Result<(), RegistryError> {
    if shutdown.is_cancelled() {
        Err(RegistryError::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_external_edit_during_a_transaction_is_not_overwritten()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("mesh.json");
        let initial = MeshFile::empty(DateTime::UNIX_EPOCH);
        fs::write(&path, serde_json::to_vec(&initial)?)?;
        let storage = Storage::open(root.path(), Path::new("mesh.json"))?;
        let external = MeshFile::empty(DateTime::UNIX_EPOCH + chrono::TimeDelta::seconds(1));
        let result = storage.change(Utc::now(), &CancellationToken::new(), |file| {
            fs::write(&path, serde_json::to_vec(&external)?)?;
            file.updated_at = Utc::now();
            Ok(())
        });
        assert!(matches!(result, Err(RegistryError::Conflict)));
        assert_eq!(storage.read()?, Some(external));
        assert!(!root.path().join("mesh.json.lock").exists());
        Ok(())
    }

    #[test]
    fn an_abandoned_lock_is_taken_over() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let storage = Storage::open(root.path(), Path::new("mesh.json"))?;
        let lock = root.path().join("mesh.json.lock");
        fs::create_dir(&lock)?;
        let old = SystemTime::now()
            .checked_sub(Duration::from_secs(120))
            .ok_or("clock before the epoch")?;
        File::open(&lock)?.set_modified(old)?;
        let written = storage.change(Utc::now(), &CancellationToken::new(), |_| Ok(()))?;
        assert_eq!(storage.read()?, Some(written));
        assert!(!lock.exists());
        Ok(())
    }

    #[test]
    fn a_live_lock_keeps_the_registry_busy() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let storage = Storage::open(root.path(), Path::new("mesh.json"))?;
        let lock = root.path().join("mesh.json.lock");
        fs::create_dir(&lock)?;
        let result = storage.change(Utc::now(), &CancellationToken::new(), |_| Ok(()));
        assert!(matches!(result, Err(RegistryError::Busy)));
        assert!(lock.exists());
        assert_eq!(storage.read()?, None);
        Ok(())
    }
}
