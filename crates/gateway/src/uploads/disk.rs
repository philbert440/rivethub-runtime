use std::collections::BTreeMap;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Component, Path};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Record, UploadError, clone_record, parse_id};
use crate::tenancy::UserId;

#[derive(Debug, Serialize, Deserialize)]
struct DiskMeta {
    owner: String,
    name: String,
    #[serde(rename = "mediaType")]
    media_type: String,
    bytes: u64,
    #[serde(rename = "expiresAt")]
    expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<String>,
}

pub(super) fn write_new(
    root: &Path,
    id: &str,
    record: &Record,
    bytes: &[u8],
) -> Result<Record, UploadError> {
    let user_dir = root.join(hash_owner(record.owner.as_str())?);
    std::fs::create_dir_all(&user_dir)?;
    contained_dir(root, &user_dir)?;
    let bin = user_dir.join(format!("{id}.{}", stored_extension(&record.name)));
    let meta_path = user_dir.join(format!("{id}.meta.json"));
    write_file(root, &user_dir, &bin, bytes)?;
    let meta = DiskMeta {
        owner: record.owner.as_str().to_owned(),
        name: record.name.clone(),
        media_type: record.media_type.clone(),
        bytes: record.bytes,
        expires_at: record.expires_at,
        session: record.session.clone(),
    };
    write_file(
        root,
        &user_dir,
        &meta_path,
        &serde_json::to_vec(&meta).map_err(|_| UploadError::Corrupt)?,
    )?;
    Ok(Record {
        bin,
        meta: meta_path,
        ..clone_record(record)
    })
}

fn write_file(root: &Path, directory: &Path, path: &Path, bytes: &[u8]) -> Result<(), UploadError> {
    if path.exists() {
        return Err(UploadError::Corrupt);
    }
    let mut temp = tempfile::Builder::new()
        .prefix(".upload-")
        .tempfile_in(directory)?;
    std::io::Write::write_all(&mut temp, bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    contained_file(root, path)?;
    Ok(())
}

pub(super) fn load(
    root: &Path,
    now: DateTime<Utc>,
) -> Result<BTreeMap<String, Record>, UploadError> {
    let mut index = BTreeMap::new();
    contained_dir(root, root)?;
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let text = name.to_str().ok_or(UploadError::Escape)?;
        if text.starts_with(".upload-") {
            std::fs::remove_file(entry.path())?;
            continue;
        }
        if !is_hex64(text) {
            return Err(UploadError::Escape);
        }
        load_owner(root, &entry.path(), text, now, &mut index)?;
    }
    Ok(index)
}

fn load_owner(
    root: &Path,
    directory: &Path,
    owner_hash: &str,
    now: DateTime<Utc>,
    index: &mut BTreeMap<String, Record>,
) -> Result<(), UploadError> {
    contained_dir(root, directory)?;
    let mut bins = BTreeMap::new();
    let mut metas = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let text = name.to_str().ok_or(UploadError::Escape)?;
        if text.starts_with(".upload-") {
            std::fs::remove_file(entry.path())?;
            continue;
        }
        if let Some(id) = text.strip_suffix(".meta.json") {
            metas.push(id.to_owned());
            continue;
        }
        let Some((id, extension)) = text.split_once('.') else {
            return Err(UploadError::Escape);
        };
        if !valid_extension(extension) {
            return Err(UploadError::Escape);
        }
        if bins.insert(id.to_owned(), text.to_owned()).is_some() {
            return Err(UploadError::Corrupt);
        }
    }
    for id in &metas {
        let bin = bins.remove(id).ok_or(UploadError::Corrupt)?;
        if let Some(record) = read_record(root, directory, owner_hash, id, &bin, now)?
            && index.insert(id.clone(), record).is_some()
        {
            return Err(UploadError::Corrupt);
        }
    }
    if !bins.is_empty() {
        return Err(UploadError::Corrupt);
    }
    Ok(())
}

fn read_record(
    root: &Path,
    directory: &Path,
    owner_hash: &str,
    id: &str,
    bin: &str,
    now: DateTime<Utc>,
) -> Result<Option<Record>, UploadError> {
    parse_id(id)?;
    let bin = directory.join(bin);
    let meta_path = directory.join(format!("{id}.meta.json"));
    contained_file(root, &bin)?;
    contained_file(root, &meta_path)?;
    let meta: DiskMeta =
        serde_json::from_slice(&std::fs::read(&meta_path)?).map_err(|_| UploadError::Corrupt)?;
    if hash_owner(&meta.owner)? != owner_hash || std::fs::metadata(&bin)?.len() != meta.bytes {
        return Err(UploadError::Escape);
    }
    let owner = UserId::new(meta.owner).map_err(|_| UploadError::Corrupt)?;
    if meta.expires_at <= now {
        remove_contained(root, &bin)?;
        remove_contained(root, &meta_path)?;
        return Ok(None);
    }
    Ok(Some(Record {
        owner,
        name: meta.name,
        media_type: meta.media_type,
        bytes: meta.bytes,
        expires_at: meta.expires_at,
        session: meta.session,
        bin,
        meta: meta_path,
    }))
}

const STORED_EXTENSION_MAX: usize = 16;

fn stored_extension(name: &str) -> String {
    match Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some(extension) if valid_extension(extension) => extension.to_ascii_lowercase(),
        Some(_) | None => "bin".to_owned(),
    }
}

fn valid_extension(extension: &str) -> bool {
    !extension.is_empty()
        && extension.len() <= STORED_EXTENSION_MAX
        && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn hash_owner(owner: &str) -> Result<String, UploadError> {
    let digest = Sha256::digest(owner.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(hex_digit(byte >> 4)?);
        encoded.push(hex_digit(byte & 0x0f)?);
    }
    Ok(encoded)
}

fn hex_digit(nibble: u8) -> Result<char, UploadError> {
    match nibble {
        0..=9 => Ok(char::from(b'0' + nibble)),
        10..=15 => Ok(char::from(b'a' + (nibble - 10))),
        _ => Err(UploadError::Corrupt),
    }
}

fn is_hex64(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn contained_dir(root: &Path, path: &Path) -> Result<(), UploadError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(UploadError::Escape);
    }
    let full = std::fs::canonicalize(path)?;
    if full == root || full.starts_with(root) {
        return Ok(());
    }
    Err(UploadError::Escape)
}

fn contained_file(root: &Path, path: &Path) -> Result<(), UploadError> {
    if path.components().any(|part| part == Component::ParentDir) {
        return Err(UploadError::Escape);
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(UploadError::Escape);
    }
    let full = std::fs::canonicalize(path)?;
    if full.starts_with(root) {
        return Ok(());
    }
    Err(UploadError::Escape)
}

pub(super) fn read_contained(root: &Path, path: &Path) -> Result<Vec<u8>, UploadError> {
    contained_file(root, path)?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes)?;
    Ok(bytes)
}

pub(super) fn remove_contained(root: &Path, path: &Path) -> Result<(), UploadError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => return Err(UploadError::Escape),
        Ok(_) => contained_file(root, path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    std::fs::remove_file(path)?;
    Ok(())
}
