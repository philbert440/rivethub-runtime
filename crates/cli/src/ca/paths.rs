use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use url::Url;

use crate::Error;

const LIMIT: usize = 1024 * 1024;
const LIMIT_PLUS: u64 = 1024 * 1024 + 1;

#[derive(Debug, Clone)]
pub(super) struct Material {
    pub(super) url: Url,
    pub(super) root: PathBuf,
    pub(super) pem: String,
}

pub(super) async fn load(root: PathBuf, url: Url) -> Result<Material, Error> {
    let (root, pem) = tokio::task::spawn_blocking(move || read_certificate(&root)).await??;
    drop(stepca::Config::new(url.clone(), pem.clone())?);
    Ok(Material { url, root, pem })
}

pub(super) fn password_file(path: &Path) -> Result<PathBuf, Error> {
    regular_file(path)
}

pub(super) fn roster_text(path: &Path) -> Result<String, Error> {
    refuse_mesh(path)?;
    reject_parent(path)?;
    let absolute = std::path::absolute(path)?;
    reject_parent(&absolute)?;
    refuse_mesh(&absolute)?;
    match fs::symlink_metadata(&absolute) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let canonical = fs::canonicalize(&absolute)?;
            refuse_mesh(&canonical)?;
            Err(Error::CaFile)
        }
        Ok(_) => {
            let canonical = regular_file(&absolute)?;
            refuse_mesh(&canonical)?;
            read_limited(&canonical)
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn check_crl_destination(path: &Path) -> Result<(), Error> {
    prepare_destination(path).map(|_| ())
}

pub(super) fn publish_crl(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let (parent, destination) = prepare_destination(path)?;
    write_atomic(&parent, &destination, bytes)
}

fn read_certificate(path: &Path) -> Result<(PathBuf, String), Error> {
    let canonical = regular_file(path)?;
    let pem = read_limited(&canonical)?;
    Ok((canonical, pem))
}

fn regular_file(path: &Path) -> Result<PathBuf, Error> {
    reject_parent(path)?;
    let absolute = std::path::absolute(path)?;
    reject_parent(&absolute)?;
    let parent = parent_of(&absolute)?;
    real_directory(parent)?;
    let metadata = fs::symlink_metadata(&absolute)?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(Error::CaFile);
    }
    let length = usize::try_from(metadata.len()).map_err(|_| Error::CaFile)?;
    if length > LIMIT {
        return Err(Error::CaFile);
    }
    let parent = fs::canonicalize(parent)?;
    let canonical = fs::canonicalize(&absolute)?;
    if canonical.starts_with(&parent) {
        Ok(canonical)
    } else {
        Err(Error::CaEscape)
    }
}

fn prepare_destination(path: &Path) -> Result<(PathBuf, PathBuf), Error> {
    reject_parent(path)?;
    let absolute = std::path::absolute(path)?;
    reject_parent(&absolute)?;
    let parent = parent_of(&absolute)?;
    real_directory(parent)?;
    let parent = fs::canonicalize(parent)?;
    let name = absolute.file_name().ok_or(Error::CaFile)?;
    let destination = parent.join(name);
    if !destination.starts_with(&parent) {
        return Err(Error::CaEscape);
    }
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.file_type().is_file() => {
            Err(Error::CaFile)
        }
        Ok(_) => {
            let canonical = fs::canonicalize(&destination)?;
            if canonical.starts_with(&parent) {
                Ok((parent, canonical))
            } else {
                Err(Error::CaEscape)
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((parent, destination)),
        Err(error) => Err(error.into()),
    }
}

fn write_atomic(parent: &Path, destination: &Path, bytes: &[u8]) -> Result<(), Error> {
    let mut temporary = tempfile::Builder::new()
        .prefix(".crl-")
        .tempfile_in(parent)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o644))?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    let file = temporary
        .persist(destination)
        .map_err(|error| Error::Io(error.error))?;
    file.sync_all()?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
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
        String::from_utf8(bytes).map_err(|_| Error::CaEncoding)
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
