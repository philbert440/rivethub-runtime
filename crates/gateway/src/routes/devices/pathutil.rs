use std::fs::File;
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use subtle::ConstantTimeEq;

use super::DeviceError;
use super::SetupError;

pub(super) fn component_ok(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && name
            .chars()
            .all(|ch| matches!(ch, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-' | '.'))
}

pub(super) fn absolute_without_parents(path: &Path) -> Result<(), SetupError> {
    if !path.is_absolute() {
        return Err(SetupError::Path);
    }
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err(SetupError::Path);
    }
    Ok(())
}

pub(super) fn tokens_equal(left: &[u8], right: &[u8]) -> bool {
    bool::from(left.ct_eq(right))
}

pub(super) enum OpenFile {
    Missing,
    Ready(PathBuf),
    Escape,
}

pub(super) fn pairing_file(stem: &str, suffix: &str) -> Option<String> {
    if component_ok(stem) && suffix == "json" {
        Some(format!("{stem}.{suffix}"))
    } else {
        None
    }
}

pub(super) fn open_pairing_file(
    root: &Path,
    stem: &str,
    suffix: &str,
) -> Result<OpenFile, DeviceError> {
    let Some(name) = pairing_file(stem, suffix) else {
        return Ok(OpenFile::Escape);
    };
    open_child(root, &name)
}

pub(super) fn open_child(root: &Path, name: &str) -> Result<OpenFile, DeviceError> {
    if !file_component(name) {
        return Ok(OpenFile::Escape);
    }
    let root_canon = match root.canonicalize() {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(OpenFile::Missing),
        Err(error) => return Err(io_registry(&error)),
    };
    let candidate = root_canon.join(name);
    match candidate.symlink_metadata() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(OpenFile::Missing),
        Err(error) => Err(io_registry(&error)),
        Ok(_) => {
            let canon = candidate
                .canonicalize()
                .map_err(|error| io_registry(&error))?;
            if stays_inside(&root_canon, &canon) {
                Ok(OpenFile::Ready(canon))
            } else {
                Ok(OpenFile::Escape)
            }
        }
    }
}

pub(super) fn stays_inside(root: &Path, target: &Path) -> bool {
    target.starts_with(root) && target != root
}

pub(super) fn ensure_dir(path: &Path) -> Result<PathBuf, DeviceError> {
    std::fs::create_dir_all(path).map_err(|error| io_registry(&error))?;
    path.canonicalize().map_err(|error| io_registry(&error))
}

pub(super) enum RosterBytes {
    Missing,
    Ready(Vec<u8>),
}

fn file_component(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
}

pub(super) fn read_roster_file(path: &Path) -> Result<RosterBytes, DeviceError> {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Err(DeviceError::Internal);
    };
    if !file_component(name) {
        return Err(DeviceError::Registry);
    }
    let Some(parent) = path.parent() else {
        return Err(DeviceError::Internal);
    };
    if !parent.is_absolute() {
        return Err(DeviceError::Registry);
    }
    let parent_canon = match parent.canonicalize() {
        Ok(canon) => canon,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RosterBytes::Missing);
        }
        Err(error) => return Err(io_registry(&error)),
    };
    match open_child(&parent_canon, name)? {
        OpenFile::Missing => Ok(RosterBytes::Missing),
        OpenFile::Escape => {
            tracing::warn!("device roster path leaves its directory");
            Err(DeviceError::Registry)
        }
        OpenFile::Ready(file) => {
            let bytes = std::fs::read(&file).map_err(|error| io_registry(&error))?;
            Ok(RosterBytes::Ready(bytes))
        }
    }
}

pub(super) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), DeviceError> {
    let Some(name) = path.file_name() else {
        return Err(DeviceError::Internal);
    };
    let Some(parent) = path.parent() else {
        return Err(DeviceError::Internal);
    };
    let parent = ensure_dir(parent)?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(".tmp");
    let tmp = parent.join(tmp_name);
    write_new(&tmp, bytes)?;
    let dest = parent.join(name);
    if let Err(error) = std::fs::rename(&tmp, &dest) {
        tracing::warn!(%error, "device registry rename failed");
        match std::fs::remove_file(&tmp) {
            Ok(()) => return Err(DeviceError::Registry),
            Err(remove_error) => {
                tracing::warn!(%remove_error, "device registry temp file remains");
                return Err(DeviceError::Registry);
            }
        }
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), DeviceError> {
    let mut file = File::create(path).map_err(|error| io_registry(&error))?;
    file.write_all(bytes).map_err(|error| io_registry(&error))?;
    file.sync_all().map_err(|error| io_registry(&error))?;
    Ok(())
}

pub(super) fn io_registry(error: &std::io::Error) -> DeviceError {
    tracing::warn!(%error, "device registry io failed");
    DeviceError::Registry
}
