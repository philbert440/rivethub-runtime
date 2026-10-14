use crate::error::{Error, refused};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

pub(crate) fn directory(path: &Path, create: bool) -> Result<PathBuf, Error> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir | Component::CurDir))
    {
        return Err(refused("CA directories must be absolute without traversal"));
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if create && current == path && !current.try_exists()? {
            fs::DirBuilder::new().mode(0o700).create(&current)?;
        }
        let metadata = fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(refused("symlinks are forbidden in CA directory paths"));
        }
    }
    Ok(fs::canonicalize(path)?)
}

pub(crate) fn contained(base: &Path, relative: &Path) -> Result<PathBuf, Error> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(refused("CA file path must be relative without traversal"));
    }
    let candidate = base.join(relative);
    let resolved = fs::canonicalize(&candidate)?;
    if !resolved.starts_with(base) {
        return Err(refused("CA file escapes its directory"));
    }
    let mut current = base.to_path_buf();
    for component in relative.components() {
        current.push(component);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            return Err(refused("symlinks are forbidden in CA file paths"));
        }
    }
    Ok(resolved)
}

pub(crate) fn read(base: &Path, relative: &Path) -> Result<String, Error> {
    let path = contained(base, relative)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > 4 * 1024 * 1024 {
        return Err(refused("CA import file must be a regular file below 4 MiB"));
    }
    let mut text = String::new();
    file.take(4 * 1024 * 1024 + 1).read_to_string(&mut text)?;
    if text.len() > 4 * 1024 * 1024 {
        return Err(refused("CA import file grew beyond 4 MiB"));
    }
    Ok(text)
}

pub(crate) fn create(base: &Path, name: &str, bytes: &[u8]) -> Result<(), Error> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(base.join(name))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub(crate) fn lock(base: &Path) -> Result<File, Error> {
    let metadata = fs::metadata(base)?;
    if metadata.permissions().mode() & 0o077 != 0
        || metadata.uid() != nix::unistd::geteuid().as_raw()
    {
        return Err(refused(
            "CA state directory must be owned by this user with mode 0700",
        ));
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .open(base.join("ca.lock"))?;
    file.try_lock()
        .map_err(|error| Error::Import(format!("CA state is already in use: {error}")))?;
    Ok(file)
}

pub(crate) fn private_key(base: &Path) -> Result<String, Error> {
    let path = contained(base, Path::new("intermediate.key.pem"))?;
    let metadata = fs::metadata(path)?;
    if metadata.permissions().mode() & 0o777 != 0o600 || metadata.nlink() != 1 {
        return Err(refused(
            "intermediate key must have mode 0600 and one hard link",
        ));
    }
    read(base, Path::new("intermediate.key.pem"))
}

pub(crate) fn replace_public(base: &Path, name: &str, bytes: &[u8]) -> Result<(), Error> {
    directory(base, false)?;
    let destination = base.join(name);
    if destination.is_symlink() {
        return Err(refused("CRL destination must not be a symlink"));
    }
    if destination.try_exists()? {
        contained(base, Path::new(name))?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(base)?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o644))?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(destination)
        .map_err(|error| Error::Io(error.error))?;
    File::open(base)?.sync_all()?;
    Ok(())
}
