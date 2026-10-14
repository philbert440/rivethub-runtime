use std::io::ErrorKind;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use wire::gateway::files::FileEntry;
use wire::gateway::files::FileKind;
use wire::gateway::files::FileList;

use super::FilesError;
use super::fence::FenceError;
use super::fence::Final;
use super::fence::Follow;
use super::fence::locate;
use super::fence::relative_text;
use super::upload::MAX_UPLOAD_BYTES;

pub(super) struct Download {
    pub name: String,
    pub file: std::fs::File,
}

pub(super) struct UploadPlan {
    pub dest: PathBuf,
    pub relative: PathBuf,
    pub temp: PathBuf,
}

pub(super) fn list(configured: &Path, raw: &str) -> Result<FileList, FilesError> {
    let root = open_root(configured)?;
    let located = locate(&root, raw, Follow::Target, Final::MustExist)
        .map_err(|error| adapt(error, FilesError::NoSuchDirectory, FilesError::ListFailed))?;
    if !located.absolute.is_dir() {
        return Err(FilesError::NoSuchDirectory);
    }
    let mut entries = Vec::new();
    for item in read_dir(&located.absolute)? {
        let item = item.map_err(|error| FilesError::ListFailed(error.to_string()))?;
        let name = entry_name(&item)?;
        if is_upload_part(&name) {
            continue;
        }
        let (kind, size, mtime) = entry_stat(&item.path(), &root)?;
        entries.push(FileEntry {
            name,
            kind,
            size,
            mtime,
        });
    }
    entries.sort_by(dir_then_name);
    Ok(FileList {
        path: path_text(&located.relative, FilesError::ListFailed)?,
        root: utf8_path(&root)?,
        entries,
    })
}

pub(super) fn download(configured: &Path, raw: &str) -> Result<Download, FilesError> {
    let root = open_root(configured)?;
    let located = locate(&root, raw, Follow::Target, Final::MustExist)
        .map_err(|error| adapt(error, FilesError::NoSuchFile, FilesError::DownloadFailed))?;
    let file = open_nofollow(&located.absolute)?;
    let meta = file
        .metadata()
        .map_err(|error| FilesError::DownloadFailed(error.to_string()))?;
    if !meta.is_file() {
        return Err(FilesError::NoSuchFile);
    }
    if meta.len() > MAX_UPLOAD_BYTES {
        return Err(FilesError::DownloadTooLarge);
    }
    let name = file_name(&located.relative)?;
    Ok(Download { name, file })
}

pub(super) fn prepare_upload(
    configured: &Path,
    dir: &str,
    name: &str,
    overwrite: bool,
    seq: u64,
) -> Result<UploadPlan, FilesError> {
    let root = open_root(configured)?;
    let parent = locate(&root, dir, Follow::Target, Final::MustExist)
        .map_err(|error| adapt(error, FilesError::NoSuchDirectory, FilesError::UploadFailed))?;
    if !parent.absolute.is_dir() {
        return Err(FilesError::NoSuchDirectory);
    }
    let dest = parent.absolute.join(name);
    let temp = parent
        .absolute
        .join(format!(".{name}.part-{}-{seq}", std::process::id()));
    if !dest.starts_with(&root) || !temp.starts_with(&root) {
        return Err(FilesError::Escape);
    }
    ensure_destination(&dest, overwrite)?;
    Ok(UploadPlan {
        dest,
        relative: parent.relative.join(name),
        temp,
    })
}

pub(super) fn finish_upload(plan: &UploadPlan, overwrite: bool) -> Result<String, FilesError> {
    ensure_destination(&plan.dest, overwrite)?;
    if overwrite {
        std::fs::rename(&plan.temp, &plan.dest)
            .map_err(|error| FilesError::UploadFailed(error.to_string()))?;
    } else {
        link_new(&plan.temp, &plan.dest)?;
    }
    path_text(&plan.relative, FilesError::UploadFailed)
}

pub(super) fn mkdir(configured: &Path, dir: &str, name: &str) -> Result<String, FilesError> {
    let root = open_root(configured)?;
    let parent = locate(&root, dir, Follow::Target, Final::MustExist)
        .map_err(|error| adapt(error, FilesError::NoSuchDirectory, FilesError::MkdirFailed))?;
    if !parent.absolute.is_dir() {
        return Err(FilesError::NoSuchDirectory);
    }
    let dest = parent.absolute.join(name);
    if !dest.starts_with(&root) {
        return Err(FilesError::Escape);
    }
    match std::fs::create_dir(&dest) {
        Ok(()) => path_text(&parent.relative.join(name), FilesError::MkdirFailed),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(FilesError::DirectoryExists),
        Err(error) => Err(FilesError::MkdirFailed(error.to_string())),
    }
}

pub(super) fn rename(configured: &Path, from: &str, to: &str) -> Result<String, FilesError> {
    let root = open_root(configured)?;
    let source = locate(&root, from, Follow::Entry, Final::MustExist)
        .map_err(|error| adapt(error, FilesError::NoSuchPath, FilesError::RenameFailed))?;
    let dest = locate(&root, to, Follow::Entry, Final::MayBeNew).map_err(|error| {
        adapt(
            error,
            FilesError::DestinationMissing,
            FilesError::RenameFailed,
        )
    })?;
    if source.relative == dest.relative {
        return Err(FilesError::SamePath);
    }
    if source.relative.as_os_str().is_empty() || dest.relative.as_os_str().is_empty() {
        return Err(FilesError::RenameRoot);
    }
    if is_directory(&source.absolute, &root)? && dest.absolute.starts_with(&source.absolute) {
        return Err(FilesError::MoveIntoSelf);
    }
    destination_parent(&dest.absolute, &root)?;
    refuse_directory_dest(&dest.absolute)?;
    std::fs::rename(&source.absolute, &dest.absolute)
        .map_err(|error| FilesError::RenameFailed(error.to_string()))?;
    path_text(&dest.relative, FilesError::RenameFailed)
}

pub(super) fn delete(configured: &Path, raw: &str, recursive: bool) -> Result<String, FilesError> {
    let root = open_root(configured)?;
    let located = locate(&root, raw, Follow::Entry, Final::MustExist)
        .map_err(|error| adapt(error, FilesError::NoSuchPath, FilesError::DeleteFailed))?;
    if located.relative.as_os_str().is_empty() {
        return Err(FilesError::DeleteRoot);
    }
    remove_entry(&located.absolute, recursive)?;
    path_text(&located.relative, FilesError::DeleteFailed)
}

pub(super) fn open_root(configured: &Path) -> Result<PathBuf, FilesError> {
    match std::fs::canonicalize(configured) {
        Ok(path) if path.is_dir() => Ok(path),
        Ok(_) => Err(FilesError::Unavailable {
            root: configured.to_path_buf(),
        }),
        Err(error) => {
            tracing::warn!(
                path = %configured.display(),
                "files root is not available: {error}"
            );
            Err(FilesError::Unavailable {
                root: configured.to_path_buf(),
            })
        }
    }
}

fn read_dir(path: &Path) -> Result<std::fs::ReadDir, FilesError> {
    std::fs::read_dir(path).map_err(|error| FilesError::ListFailed(error.to_string()))
}

fn entry_name(item: &std::fs::DirEntry) -> Result<String, FilesError> {
    match item.file_name().into_string() {
        Ok(name) => Ok(name),
        Err(raw) => Err(FilesError::ListFailed(raw.to_string_lossy().into_owned())),
    }
}

fn entry_stat(path: &Path, root: &Path) -> Result<(FileKind, u64, i64), FilesError> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|error| FilesError::ListFailed(error.to_string()))?;
    if meta.file_type().is_symlink() {
        return link_stat(path, root, &meta);
    }
    let kind = if meta.is_dir() {
        FileKind::Dir
    } else {
        FileKind::File
    };
    Ok((kind, meta.len(), modified_ms(&meta)?))
}

fn link_stat(
    path: &Path,
    root: &Path,
    link: &std::fs::Metadata,
) -> Result<(FileKind, u64, i64), FilesError> {
    match std::fs::canonicalize(path) {
        Ok(target) if target.starts_with(root) => target_stat(&target),
        Ok(_) => unresolved_link(link),
        Err(error) if error.kind() == ErrorKind::NotFound => unresolved_link(link),
        Err(error) => Err(FilesError::ListFailed(error.to_string())),
    }
}

fn target_stat(target: &Path) -> Result<(FileKind, u64, i64), FilesError> {
    let meta =
        std::fs::metadata(target).map_err(|error| FilesError::ListFailed(error.to_string()))?;
    let kind = if meta.is_dir() {
        FileKind::Dir
    } else {
        FileKind::File
    };
    Ok((kind, meta.len(), modified_ms(&meta)?))
}

fn unresolved_link(link: &std::fs::Metadata) -> Result<(FileKind, u64, i64), FilesError> {
    Ok((FileKind::File, link.len(), modified_ms(link)?))
}

fn modified_ms(meta: &std::fs::Metadata) -> Result<i64, FilesError> {
    let modified = meta
        .modified()
        .map_err(|error| FilesError::ListFailed(error.to_string()))?;
    let since = modified
        .duration_since(UNIX_EPOCH)
        .map_err(|error| FilesError::ListFailed(error.to_string()))?;
    i64::try_from(since.as_millis()).map_err(|error| FilesError::ListFailed(error.to_string()))
}

fn dir_then_name(left: &FileEntry, right: &FileEntry) -> std::cmp::Ordering {
    dir_rank(left.kind)
        .cmp(&dir_rank(right.kind))
        .then_with(|| left.name.cmp(&right.name))
}

fn dir_rank(kind: FileKind) -> u8 {
    match kind {
        FileKind::Dir => 0,
        FileKind::File => 1,
    }
}

fn open_nofollow(path: &Path) -> Result<std::fs::File, FilesError> {
    match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == ErrorKind::NotFound => Err(FilesError::NoSuchFile),
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => Err(FilesError::Escape),
        Err(error) => Err(FilesError::DownloadFailed(error.to_string())),
    }
}

fn file_name(relative: &Path) -> Result<String, FilesError> {
    let Some(name) = relative.file_name() else {
        return Err(FilesError::NoSuchFile);
    };
    match name.to_str() {
        Some(text) => Ok(text.to_owned()),
        None => Err(FilesError::DownloadFailed(
            "file name is not utf-8".to_owned(),
        )),
    }
}

fn ensure_destination(dest: &Path, overwrite: bool) -> Result<(), FilesError> {
    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.is_dir() => Err(FilesError::DirectoryExists),
        Ok(_) if !overwrite => Err(FilesError::FileExists),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(FilesError::UploadFailed(error.to_string())),
    }
}

fn link_new(temp: &Path, dest: &Path) -> Result<(), FilesError> {
    match std::fs::hard_link(temp, dest) {
        Ok(()) => match std::fs::remove_file(temp) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(FilesError::UploadFailed(error.to_string())),
        },
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(FilesError::FileExists),
        Err(error) => Err(FilesError::UploadFailed(error.to_string())),
    }
}

fn refuse_directory_dest(dest: &Path) -> Result<(), FilesError> {
    match std::fs::symlink_metadata(dest) {
        Ok(meta) if meta.is_dir() => Err(FilesError::DirectoryExists),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(FilesError::RenameFailed(error.to_string())),
    }
}

fn destination_parent(dest: &Path, root: &Path) -> Result<(), FilesError> {
    let Some(parent) = dest.parent() else {
        return Err(FilesError::DestinationMissing);
    };
    if parent.starts_with(root) && parent.is_dir() {
        Ok(())
    } else {
        Err(FilesError::DestinationMissing)
    }
}

fn is_directory(path: &Path, root: &Path) -> Result<bool, FilesError> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|error| FilesError::RenameFailed(error.to_string()))?;
    if meta.is_dir() {
        return Ok(true);
    }
    if !meta.file_type().is_symlink() {
        return Ok(false);
    }
    match std::fs::canonicalize(path) {
        Ok(target) if target.starts_with(root) => Ok(target.is_dir()),
        Ok(_) => Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(FilesError::RenameFailed(error.to_string())),
    }
}

fn remove_entry(path: &Path, recursive: bool) -> Result<(), FilesError> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|error| FilesError::DeleteFailed(error.to_string()))?;
    if !meta.is_dir() {
        return std::fs::remove_file(path)
            .map_err(|error| FilesError::DeleteFailed(error.to_string()));
    }
    if recursive {
        return std::fs::remove_dir_all(path)
            .map_err(|error| FilesError::DeleteFailed(error.to_string()));
    }
    if directory_empty(path)? {
        return std::fs::remove_dir(path)
            .map_err(|error| FilesError::DeleteFailed(error.to_string()));
    }
    Err(FilesError::NotEmpty)
}

fn directory_empty(path: &Path) -> Result<bool, FilesError> {
    let mut entries =
        std::fs::read_dir(path).map_err(|error| FilesError::DeleteFailed(error.to_string()))?;
    match entries.next() {
        None => Ok(true),
        Some(Ok(_)) => Ok(false),
        Some(Err(error)) => Err(FilesError::DeleteFailed(error.to_string())),
    }
}

fn utf8_path(path: &Path) -> Result<String, FilesError> {
    match path.to_str() {
        Some(text) => Ok(text.to_owned()),
        None => Err(FilesError::ListFailed("files root is not utf-8".to_owned())),
    }
}

fn path_text(relative: &Path, failed: fn(String) -> FilesError) -> Result<String, FilesError> {
    match relative_text(relative) {
        Ok(text) => Ok(text),
        Err(error) => Err(adapt(error, FilesError::Escape, failed)),
    }
}

fn adapt(error: FenceError, missing: FilesError, failed: fn(String) -> FilesError) -> FilesError {
    match error {
        FenceError::Escape => FilesError::Escape,
        FenceError::Missing => missing,
        FenceError::Io(error) => failed(error.to_string()),
    }
}

fn is_upload_part(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.') else {
        return false;
    };
    let Some((_, tail)) = rest.rsplit_once(".part-") else {
        return false;
    };
    let Some((left, right)) = tail.split_once('-') else {
        return false;
    };
    !left.is_empty()
        && !right.is_empty()
        && left.bytes().all(|byte| byte.is_ascii_digit())
        && right.bytes().all(|byte| byte.is_ascii_digit())
}
