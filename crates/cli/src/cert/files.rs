use crate::Error;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

pub(super) fn absolute_path(path: &Path) -> Result<PathBuf, Error> {
    reject_parent(path)?;
    let absolute = std::path::absolute(path)?;
    reject_parent(&absolute)?;
    if absolute.is_absolute() {
        Ok(absolute)
    } else {
        Err(Error::PkiInput("holder path must be absolute"))
    }
}

pub(super) fn certificate_stored(state: &Path, name: &str) -> Result<bool, Error> {
    let state = absolute_path(state)?;
    if !existing_directory(&state)? {
        return Ok(false);
    }
    let tls = state.join("tls");
    if !tls.starts_with(&state) {
        return Err(Error::PkiInput(
            "holder directory escapes its state directory",
        ));
    }
    if !existing_directory(&tls)? {
        return Ok(false);
    }
    let certificate = named(&tls, name)?;
    match fs::symlink_metadata(&certificate) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            Error::PkiInput("holder certificate must be a regular file, not a symlink"),
        ),
        Ok(_) => {
            let canonical = fs::canonicalize(&certificate)?;
            if canonical.starts_with(&tls) {
                Ok(true)
            } else {
                Err(Error::PkiInput("holder file escapes its directory"))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn tls_directory(state: &Path) -> Result<PathBuf, Error> {
    let state = directory(state)?;
    let tls = state.join("tls");
    let tls = directory(&tls)?;
    if tls.starts_with(&state) {
        private_directory(&tls)?;
        Ok(tls)
    } else {
        Err(Error::PkiInput(
            "holder directory escapes its state directory",
        ))
    }
}

pub(super) fn read_external(path: &Path) -> Result<String, Error> {
    let absolute = absolute_path(path)?;
    let parent = absolute
        .parent()
        .ok_or(Error::PkiInput("holder input requires a parent directory"))?;
    let parent = real_directory(parent)?;
    let metadata = fs::symlink_metadata(&absolute)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Error::PkiInput(
            "holder input must be a regular file, not a symlink",
        ));
    }
    let canonical = fs::canonicalize(&absolute)?;
    if canonical.starts_with(&parent) {
        read_limited(&canonical)
    } else {
        Err(Error::PkiInput("holder input escapes its parent directory"))
    }
}

pub(super) fn read_key(directory: &Path, name: &str) -> Result<String, Error> {
    let path = contained(directory, name)?;
    let metadata = fs::metadata(&path)?;
    if metadata.permissions().mode() & 0o777 != 0o600 || metadata.nlink() != 1 {
        return Err(Error::PkiInput(
            "holder key must have mode 0600 and one hard link",
        ));
    }
    let pem = read_limited(&path)?;
    if pem.matches("-----BEGIN ").count() == 1 {
        Ok(pem)
    } else {
        Err(Error::PkiInput("holder key must be one PEM private key"))
    }
}

pub(super) fn read_certificate(directory: &Path, name: &str) -> Result<Option<String>, Error> {
    let destination = named(directory, name)?;
    match destination.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(Error::PkiInput("holder certificate must not be a symlink"))
        }
        Ok(metadata) if !metadata.is_file() => {
            Err(Error::PkiInput("holder certificate must be a regular file"))
        }
        Ok(_) => {
            let path = contained(directory, name)?;
            Ok(Some(read_limited(&path)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn publish(directory: &Path, name: &str, bytes: &[u8]) -> Result<(), Error> {
    private_directory(directory)?;
    let destination = named(directory, name)?;
    match destination.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(Error::PkiInput(
                "holder output must replace a regular file, not a symlink",
            ));
        }
        Ok(_) => {
            let resolved = fs::canonicalize(&destination)?;
            if !resolved.starts_with(directory) {
                return Err(Error::PkiInput("holder output escapes its directory"));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut temporary = tempfile::Builder::new()
        .prefix(".holder-")
        .tempfile_in(directory)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    let file = temporary
        .persist(&destination)
        .map_err(|error| Error::Io(error.error))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.sync_all()?;
    let metadata = fs::metadata(&destination)?;
    if metadata.permissions().mode() & 0o777 != 0o600 || metadata.nlink() != 1 {
        return Err(Error::PkiInput(
            "holder output must have mode 0600 and one hard link",
        ));
    }
    File::open(directory)?.sync_all()?;
    Ok(())
}

pub(super) fn lock(directory: &Path) -> Result<File, Error> {
    private_directory(directory)?;
    let path = named(directory, "renew.lock")?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(path)?;
    file.try_lock()
        .map_err(|_| Error::PkiInput("holder state is already in use"))?;
    Ok(file)
}

fn directory(path: &Path) -> Result<PathBuf, Error> {
    let absolute = absolute_path(path)?;
    let mut current = PathBuf::new();
    for component in absolute.components() {
        current.push(component);
        if !current.try_exists()? {
            fs::DirBuilder::new().mode(0o700).create(&current)?;
            fs::set_permissions(&current, fs::Permissions::from_mode(0o700))?;
        }
        let metadata = fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::PkiInput("symlinks are forbidden in the holder path"));
        }
    }
    let canonical = fs::canonicalize(&absolute)?;
    if canonical == current {
        Ok(canonical)
    } else {
        Err(Error::PkiInput(
            "holder path resolves outside its directory",
        ))
    }
}

fn existing_directory(path: &Path) -> Result<bool, Error> {
    let absolute = absolute_path(path)?;
    let mut current = PathBuf::new();
    for component in absolute.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(Error::PkiInput("symlinks are forbidden in the holder path"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    }
    let canonical = fs::canonicalize(&absolute)?;
    if canonical == current {
        Ok(true)
    } else {
        Err(Error::PkiInput(
            "holder path resolves outside its directory",
        ))
    }
}

fn real_directory(path: &Path) -> Result<PathBuf, Error> {
    let absolute = absolute_path(path)?;
    let mut current = PathBuf::new();
    for component in absolute.components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(Error::PkiInput("symlinks are forbidden in the holder path"));
        }
    }
    let canonical = fs::canonicalize(&absolute)?;
    if canonical == current {
        Ok(canonical)
    } else {
        Err(Error::PkiInput(
            "holder path resolves outside its directory",
        ))
    }
}

fn private_directory(path: &Path) -> Result<(), Error> {
    let metadata = fs::metadata(path)?;
    if metadata.uid() == nix::unistd::Uid::effective().as_raw()
        && metadata.permissions().mode().trailing_zeros() >= 6
        && metadata.is_dir()
    {
        Ok(())
    } else {
        Err(Error::PkiInput(
            "holder directory must be owned by this user and not group-accessible",
        ))
    }
}

fn contained(directory: &Path, name: &str) -> Result<PathBuf, Error> {
    let destination = named(directory, name)?;
    let metadata = fs::symlink_metadata(&destination)?;
    if metadata.file_type().is_symlink() {
        return Err(Error::PkiInput("symlinks are forbidden in holder files"));
    }
    let resolved = fs::canonicalize(&destination)?;
    if resolved.starts_with(directory) {
        Ok(resolved)
    } else {
        Err(Error::PkiInput("holder file escapes its directory"))
    }
}

fn named(directory: &Path, name: &str) -> Result<PathBuf, Error> {
    let relative = Path::new(name);
    let mut components = relative.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(Error::PkiInput(
            "holder file name must be a single path component",
        ));
    }
    Ok(directory.join(relative))
}

fn read_limited(path: &Path) -> Result<String, Error> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > 1024 * 1024 {
        return Err(Error::PkiInput(
            "holder input must be a regular file of at most 1 MiB",
        ));
    }
    let mut text = String::new();
    file.take(1024 * 1024 + 1).read_to_string(&mut text)?;
    if text.len() > 1024 * 1024 {
        Err(Error::PkiInput("holder input grew past 1 MiB"))
    } else {
        Ok(text)
    }
}

fn reject_parent(path: &Path) -> Result<(), Error> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        Err(Error::PkiInput("path traversal is refused"))
    } else {
        Ok(())
    }
}
