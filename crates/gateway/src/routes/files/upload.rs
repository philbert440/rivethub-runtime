use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

use http_body::Body;
use tokio::io::AsyncWriteExt;

use super::FilesError;

pub(super) const MAX_UPLOAD_BYTES: u64 = 1024 * 1024 * 1024;

static UPLOAD_SEQ: AtomicU64 = AtomicU64::new(1);

pub(super) fn next_seq() -> u64 {
    UPLOAD_SEQ.fetch_add(1, Ordering::Relaxed)
}

pub(super) fn admit(total: u64, addition: u64) -> Result<u64, FilesError> {
    let Some(sum) = total.checked_add(addition) else {
        return Err(FilesError::TooLarge);
    };
    if sum > MAX_UPLOAD_BYTES {
        return Err(FilesError::TooLarge);
    }
    Ok(sum)
}

pub(super) struct TempUpload {
    path: PathBuf,
}

impl TempUpload {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TempUpload {
    fn drop(&mut self) {
        let path = std::mem::take(&mut self.path);
        let cleanup = move || remove_temp(&path);
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(cleanup);
            }
            Err(_outside) => cleanup(),
        }
    }
}

fn remove_temp(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(path = %path.display(), "upload temp cleanup failed: {error}");
        }
    }
}

pub(super) async fn write_capped(
    path: &Path,
    body: &mut axum::body::Body,
) -> Result<(), FilesError> {
    let mut file = match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .await
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(temp_collision(path).await);
        }
        Err(error) => return Err(FilesError::UploadFailed(error.to_string())),
    };
    let mut total = 0_u64;
    loop {
        let polled = std::future::poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await;
        let Some(frame) = polled else {
            break;
        };
        let frame = frame.map_err(|error| FilesError::UploadFailed(error.to_string()))?;
        match frame.into_data() {
            Ok(data) => {
                let Ok(len) = u64::try_from(data.len()) else {
                    return Err(FilesError::TooLarge);
                };
                total = admit(total, len)?;
                file.write_all(&data)
                    .await
                    .map_err(|error| FilesError::UploadFailed(error.to_string()))?;
            }
            Err(trailer) => drop(trailer),
        }
    }
    file.sync_all()
        .await
        .map_err(|error| FilesError::UploadFailed(error.to_string()))?;
    Ok(())
}

async fn temp_collision(path: &Path) -> FilesError {
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_symlink() => FilesError::Escape,
        Ok(_) => FilesError::UploadFailed("upload temp already exists".to_owned()),
        Err(error) => FilesError::UploadFailed(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::MAX_UPLOAD_BYTES;
    use super::admit;

    #[test]
    fn the_cap_is_one_gibibyte_on_both_sides() {
        assert_eq!(
            admit(0, MAX_UPLOAD_BYTES).expect("the cap itself"),
            MAX_UPLOAD_BYTES
        );
        assert!(admit(0, MAX_UPLOAD_BYTES + 1).is_err());
        assert!(admit(MAX_UPLOAD_BYTES, 1).is_err());
        assert!(admit(u64::MAX, 1).is_err());
    }

    #[tokio::test]
    async fn drop_removes_the_temp_on_the_blocking_pool() {
        let keep = tempfile::tempdir().expect("temp dir");
        let path = keep.path().join(".n.part-1-1");
        std::fs::write(&path, b"x").expect("seed");
        drop(super::TempUpload::new(path.clone()));
        let removed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while path.exists() {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(removed.is_ok(), "temp file still present");
    }
}
