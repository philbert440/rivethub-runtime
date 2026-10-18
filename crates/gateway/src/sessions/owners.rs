use std::collections::BTreeMap;
use std::fs;
use std::io::{ErrorKind, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use tempfile::NamedTempFile;
use wire::JsonObject;
use wire::harness::HarnessId;

use super::session_key;
use crate::tenancy::UserId;

pub use config::paths::SESSION_OWNERS_FILE_NAME as OWNERS_FILE_NAME;
const OWNERS_FILE_MODE: u32 = 0o600;

#[must_use]
pub fn owners_file(state_dir: &Path) -> PathBuf {
    config::paths::session_owners_file(state_dir)
}

#[derive(Debug, thiserror::Error)]
pub(super) enum OwnersFileError {
    #[error("session owners file I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("session owners file is not a JSON object of user ids: {0}")]
    Json(#[from] serde_json::Error),
    #[error("session owners file {0} has no parent directory")]
    NoParent(PathBuf),
}

#[derive(Debug, Default)]
pub(super) struct Folded {
    pub(super) native: BTreeMap<String, UserId>,
    pub(super) verbatim: BTreeMap<String, UserId>,
}

#[derive(Debug)]
pub(super) struct OwnersFile {
    path: PathBuf,
    verbatim: BTreeMap<String, UserId>,
    writing: Mutex<()>,
}

impl OwnersFile {
    pub(super) fn open(path: PathBuf) -> (Self, BTreeMap<String, UserId>) {
        let folded = match read(&path) {
            Ok(Some(document)) => fold(&document),
            Ok(None) => Folded::default(),
            Err(error) => unreadable(&path, &error),
        };
        (
            Self {
                path,
                verbatim: folded.verbatim,
                writing: Mutex::new(()),
            },
            folded.native,
        )
    }

    pub(super) fn persist_best_effort(&self, snapshot: impl FnOnce() -> BTreeMap<String, UserId>) {
        let writing = self.writing.lock();
        let native = snapshot();
        if let Err(error) = self.write(&native) {
            tracing::error!(
                path = %self.path.display(),
                %error,
                "session owners were not persisted; the change stays in memory"
            );
        }
        drop(writing);
    }

    fn write(&self, native: &BTreeMap<String, UserId>) -> Result<(), OwnersFileError> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| OwnersFileError::NoParent(self.path.clone()))?;
        fs::create_dir_all(parent)?;
        let document: BTreeMap<&str, &str> = self
            .verbatim
            .iter()
            .chain(native)
            .map(|(key, user)| (key.as_str(), user.as_str()))
            .collect();
        let mut bytes = serde_json::to_vec_pretty(&document)?;
        bytes.push(b'\n');
        let mut temporary = NamedTempFile::new_in(parent)?;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(OWNERS_FILE_MODE))?;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|error| OwnersFileError::Io(error.error))?;
        Ok(())
    }
}

fn read(path: &Path) -> Result<Option<JsonObject>, OwnersFileError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn unreadable(path: &Path, error: &OwnersFileError) -> Folded {
    tracing::error!(
        path = %path.display(),
        %error,
        "session owners file is unreadable; starting with no owners"
    );
    Folded::default()
}

struct FileKey {
    native: String,
    canonical: bool,
}

fn fold(document: &JsonObject) -> Folded {
    let mut folded = Folded::default();
    let mut canonical = BTreeMap::new();
    for (raw, value) in document.as_map() {
        let Some(user) = stored_user(raw, value.as_str()) else {
            continue;
        };
        let Some(key) = file_key(raw) else {
            folded.verbatim.insert(raw.clone(), user);
            continue;
        };
        let held = canonical.get(&key.native).copied();
        if held.is_none() || (key.canonical && held == Some(false)) {
            canonical.insert(key.native.clone(), key.canonical);
            folded.native.insert(key.native, user);
        }
    }
    folded
}

fn stored_user(raw: &str, value: Option<&str>) -> Option<UserId> {
    if raw.is_empty() {
        return None;
    }
    let value = value?;
    if value.is_empty() {
        return None;
    }
    if let Ok(user) = UserId::new(value.to_owned()) {
        Some(user)
    } else {
        tracing::warn!(
            key = raw,
            "session owner entry has an invalid user id; dropped"
        );
        None
    }
}

fn file_key(raw: &str) -> Option<FileKey> {
    let canonical = raw
        .split_once(':')
        .is_some_and(|(prefix, _)| prefix.parse::<HarnessId>().is_ok());
    match session_key(&collapse_path_fallback(raw)) {
        Ok(native) => Some(FileKey { native, canonical }),
        Err(error) => {
            tracing::debug!(key = raw, %error, "session owner key kept verbatim");
            None
        }
    }
}

fn collapse_path_fallback(raw: &str) -> String {
    if let Some((prefix, native)) = raw.split_once(':')
        && prefix.parse::<HarnessId>().is_ok()
        && let Some((_, tail)) = native.rsplit_once('/')
        && tail.len() == 36
        && uuid::Uuid::try_parse(tail).is_ok()
    {
        format!("{prefix}:{tail}")
    } else {
        raw.to_owned()
    }
}
