use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path, PathBuf};

use crate::Error;

const LIMIT: usize = 1024 * 1024;
const LIMIT_PLUS: u64 = 1024 * 1024 + 1;

pub(crate) fn read_root(path: &Path) -> Result<(PathBuf, String), Error> {
    let canonical = contained_file(path, false)?;
    let pem = read_limited(&canonical)?;
    Ok((canonical, pem))
}

pub(crate) fn contained_file(path: &Path, roster: bool) -> Result<PathBuf, Error> {
    if roster {
        refuse_mesh(path)?;
    }
    reject_parent(path)?;
    let absolute = std::path::absolute(path)?;
    reject_parent(&absolute)?;
    if roster {
        refuse_mesh(&absolute)?;
    }
    let parent = parent_of(&absolute)?;
    real_directory(parent)?;
    let metadata = match fs::symlink_metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(Error::Missing),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(Error::CaFile);
    }
    let length = usize::try_from(metadata.len()).map_err(|_| Error::CaFile)?;
    if length > LIMIT {
        return Err(Error::CaFile);
    }
    let parent = fs::canonicalize(parent)?;
    let canonical = match fs::canonicalize(&absolute) {
        Ok(canonical) => canonical,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(Error::Missing),
        Err(error) => return Err(error.into()),
    };
    if roster {
        refuse_mesh(&canonical)?;
    }
    if canonical.starts_with(&parent) {
        Ok(canonical)
    } else {
        Err(Error::CaEscape)
    }
}

pub(crate) fn read_roster(path: &Path) -> Result<String, Error> {
    let canonical = contained_file(path, true)?;
    read_limited(&canonical)
}

fn read_limited(path: &Path) -> Result<String, Error> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(path)?;
    let metadata = file.metadata()?;
    let length = usize::try_from(metadata.len()).map_err(|_| Error::CaFile)?;
    if !metadata.file_type().is_file() || length > LIMIT {
        return Err(Error::CaFile);
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(LIMIT_PLUS)
        .read_to_end(&mut bytes)?;
    if bytes.len() > LIMIT {
        Err(Error::CaFile)
    } else {
        String::from_utf8(bytes).map_err(|_| Error::Root)
    }
}

fn real_directory(path: &Path) -> Result<(), Error> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() {
            return Err(Error::CaEscape);
        }
        if !metadata.is_dir() {
            return Err(Error::CaFile);
        }
    }
    Ok(())
}

fn parent_of(path: &Path) -> Result<&Path, Error> {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or(Error::CaFile)
}

fn reject_parent(path: &Path) -> Result<(), Error> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        Err(Error::CaTraversal)
    } else {
        Ok(())
    }
}

fn refuse_mesh(path: &Path) -> Result<(), Error> {
    if path.file_name().is_some_and(|name| name == "mesh.json") {
        Err(Error::MeshRegistry)
    } else {
        Ok(())
    }
}
