use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;

use crate::StateError;

pub(super) fn check(directory: &Path) -> Result<Vec<File>, StateError> {
    let entries = std::fs::read_dir(directory).map_err(|source| StateError::Inspect {
        path: directory.to_owned(),
        source,
    })?;
    let mut locks = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| StateError::Inspect {
            path: directory.to_owned(),
            source,
        })?;
        let path = entry.path();
        let extension = path.extension().and_then(std::ffi::OsStr::to_str);
        if !matches!(extension, Some("pid" | "lock")) {
            continue;
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
            .open(&path)
            .map_err(|source| StateError::Inspect {
                path: path.clone(),
                source,
            })?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(StateError::Locked { path }),
            Err(std::fs::TryLockError::Error(source)) => {
                return Err(StateError::Inspect { path, source });
            }
        }
        let mut text = String::new();
        Read::by_ref(&mut file)
            .take(1025)
            .read_to_string(&mut text)
            .map_err(|source| StateError::Inspect {
                path: path.clone(),
                source,
            })?;
        if text.len() > 1024 {
            return Err(StateError::InvalidOwner { path });
        }
        if text.trim().is_empty() && extension == Some("lock") {
            locks.push(file);
            continue;
        }
        let pid = text
            .trim()
            .parse::<u32>()
            .map_err(|_| StateError::InvalidOwner { path: path.clone() })?;
        let raw =
            i32::try_from(pid).map_err(|_| StateError::InvalidOwner { path: path.clone() })?;
        if raw <= 0 {
            return Err(StateError::InvalidOwner { path });
        }
        match kill(Pid::from_raw(raw), None) {
            Ok(()) | Err(Errno::EPERM) => return Err(StateError::LiveOwner { path, pid }),
            Err(Errno::ESRCH) => locks.push(file),
            Err(error) => {
                return Err(StateError::Inspect {
                    path,
                    source: error.into(),
                });
            }
        }
    }
    Ok(locks)
}
