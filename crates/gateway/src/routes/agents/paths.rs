use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use super::error::AgentError;

const LINK_NAME: &str = "rivet-shared";

pub(crate) fn registry_file(state_dir: &Path) -> Result<PathBuf, AgentError> {
    let root = canonical_dir(state_dir)?;
    let file = root.join("agents.json");
    refuse_escape(&root, &file)?;
    if is_symlink(&file) {
        tracing::warn!(path = %file.display(), "agent registry file is a symlink");
        return Err(AgentError::Unavailable);
    }
    Ok(file)
}

pub(crate) fn prepare_directory(path: &Path) -> Result<(), AgentError> {
    let normalized = super::check::directory_path(path)?;
    if !stays_on_requested_path(&normalized)? {
        return Err(AgentError::BadRequest(
            "directory escapes its requested path",
        ));
    }
    match std::fs::symlink_metadata(&normalized) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(AgentError::BadRequest(
                "directory escapes its requested path",
            ));
        }
        Ok(meta) if meta.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(AgentError::Directory(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "agent directory path exists and is not a directory",
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(AgentError::Directory(error)),
    }
    std::fs::create_dir_all(&normalized).map_err(AgentError::Directory)?;
    if !stays_on_requested_path(&normalized)? {
        return Err(AgentError::BadRequest(
            "directory escapes its requested path",
        ));
    }
    let meta = std::fs::symlink_metadata(&normalized).map_err(AgentError::Directory)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(AgentError::BadRequest(
            "directory escapes its requested path",
        ));
    }
    tracing::info!(directory = %normalized.display(), "created agent directory");
    Ok(())
}

pub(crate) fn ensure_shared_link(
    dir: &Path,
    shared: &Path,
    enabled: bool,
) -> Result<(), AgentError> {
    if !stays_on_requested_path(dir)? {
        return Err(AgentError::BadRequest(
            "directory escapes its requested path",
        ));
    }
    let link = contained_child(dir, LINK_NAME)?;
    if enabled {
        create_link(&link, shared)
    } else {
        remove_our_link(&link, shared)
    }
}

pub(crate) fn write_file(file: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    let parent = file.parent().ok_or(AgentError::Unavailable)?;
    let temp = file.with_extension("json.tmp");
    refuse_escape(parent, &temp)?;
    if let Ok(meta) = std::fs::symlink_metadata(&temp)
        && meta.file_type().is_symlink()
    {
        tracing::warn!(path = %temp.display(), "agent registry temp file is a symlink");
        return Err(AgentError::Unavailable);
    }
    write_temp(&temp, bytes)?;
    std::fs::rename(&temp, file).map_err(|error| store_io(&error))?;
    let dir = File::open(parent).map_err(|error| store_io(&error))?;
    dir.sync_all().map_err(|error| store_io(&error))?;
    Ok(())
}

fn create_link(link: &Path, shared: &Path) -> Result<(), AgentError> {
    match std::fs::symlink_metadata(link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(link).map_err(AgentError::Directory)?;
            if target == shared {
                Ok(())
            } else {
                Err(AgentError::Directory(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "rivet-shared points elsewhere",
                )))
            }
        }
        Ok(_) => Err(AgentError::Directory(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "rivet-shared exists and is not a symlink",
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::os::unix::fs::symlink(shared, link).map_err(AgentError::Directory)
        }
        Err(error) => Err(AgentError::Directory(error)),
    }
}

fn remove_our_link(link: &Path, shared: &Path) -> Result<(), AgentError> {
    match std::fs::symlink_metadata(link) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AgentError::Directory(error)),
        Ok(meta) if meta.file_type().is_symlink() => {
            let target = std::fs::read_link(link).map_err(AgentError::Directory)?;
            if target == shared {
                std::fs::remove_file(link).map_err(AgentError::Directory)
            } else {
                Ok(())
            }
        }
        Ok(_) => Ok(()),
    }
}

fn contained_child(dir: &Path, name: &str) -> Result<PathBuf, AgentError> {
    let child = dir.join(name);
    if super::check::contained(dir, &child)? {
        Ok(child)
    } else {
        Err(AgentError::BadRequest(
            "directory escapes its requested path",
        ))
    }
}

fn refuse_escape(root: &Path, child: &Path) -> Result<(), AgentError> {
    match super::check::contained(root, child) {
        Ok(true) => Ok(()),
        Ok(false) => Err(AgentError::Unavailable),
        Err(error) => {
            tracing::warn!(%error, path = %child.display(), "agent path is not contained");
            Err(AgentError::Unavailable)
        }
    }
}

fn stays_on_requested_path(path: &Path) -> Result<bool, AgentError> {
    let mut cursor = PathBuf::new();
    for component in path.components() {
        cursor.push(component);
        match std::fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.file_type().is_symlink() => match std::fs::canonicalize(&cursor) {
                Ok(landed) if landed == cursor => return Ok(true),
                Ok(landed) => {
                    tracing::warn!(
                        path = %cursor.display(),
                        canonical = %landed.display(),
                        "agent directory symlink resolves outside the requested directory"
                    );
                    return Ok(false);
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        path = %cursor.display(),
                        "agent directory symlink did not resolve"
                    );
                    return Ok(false);
                }
            },
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(error) => return Err(AgentError::Directory(error)),
        }
    }
    match std::fs::canonicalize(path) {
        Ok(canonical) => Ok(canonical == path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(AgentError::Directory(error)),
    }
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

fn canonical_dir(path: &Path) -> Result<PathBuf, AgentError> {
    std::fs::create_dir_all(path).map_err(|error| store_io(&error))?;
    std::fs::canonicalize(path).map_err(|error| store_io(&error))
}

fn write_temp(temp: &Path, bytes: &[u8]) -> Result<(), AgentError> {
    let mut out = File::create(temp).map_err(|error| store_io(&error))?;
    out.write_all(bytes).map_err(|error| store_io(&error))?;
    out.sync_all().map_err(|error| store_io(&error))?;
    Ok(())
}

fn store_io(error: &std::io::Error) -> AgentError {
    tracing::warn!(%error, "agent registry file operation failed");
    AgentError::Unavailable
}
