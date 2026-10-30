use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use crate::error::ApiError;

pub(super) async fn requested(root: Option<&Path>, working: &str) -> Result<PathBuf, ApiError> {
    contained(root.ok_or_else(root_required)?, Path::new(working)).await
}

pub(super) async fn configured(
    root: Option<&Path>,
    working: Option<&Path>,
) -> Result<PathBuf, ApiError> {
    match working {
        Some(working) if working.is_absolute() => lookup(working.to_path_buf()).await,
        Some(working) => contained(root.ok_or_else(root_required)?, working).await,
        None => {
            let root = root.ok_or_else(root_required)?;
            contained(root, root).await
        }
    }
}

async fn contained(root: &Path, working: &Path) -> Result<PathBuf, ApiError> {
    if root.as_os_str().is_empty() || working.as_os_str().is_empty() {
        return Err(root_required());
    }
    if working
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(escape());
    }
    let joined = if working.is_absolute() {
        working.to_path_buf()
    } else {
        root.join(working)
    };
    let found = lookup(joined).await?;
    if found.starts_with(lookup(root.to_path_buf()).await?) {
        Ok(found)
    } else {
        Err(escape())
    }
}

async fn lookup(path: PathBuf) -> Result<PathBuf, ApiError> {
    match tokio::task::spawn_blocking(move || std::fs::canonicalize(path))
        .await
        .map_err(ApiError::Task)?
    {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(ApiError::BadRequest(
            "the task's working directory does not exist on its node".into(),
        )),
        Err(error) => Err(ApiError::Io(error)),
    }
}

fn root_required() -> ApiError {
    ApiError::Conflict(
        "this node has no files root or runtime.workspace for a task's working directory".into(),
    )
}

fn escape() -> ApiError {
    ApiError::Forbidden("the task's working directory is outside the node's files root".into())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::configured;
    use super::requested;

    #[tokio::test]
    async fn a_requested_directory_stays_inside_the_root_and_a_configured_one_may_not() {
        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("files");
        let outside = parent.path().join("outside");
        std::fs::create_dir_all(root.join("work")).expect("work");
        std::fs::create_dir(&outside).expect("outside");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("link");
        let inside = requested(Some(&root), "work").await.expect("inside");
        assert_eq!(inside, root.join("work").canonicalize().expect("canonical"));
        assert!(requested(Some(&root), "link").await.is_err());
        assert!(requested(Some(&root), "work/..").await.is_err());
        assert!(requested(None, "work").await.is_err());
        let absolute = configured(Some(&root), Some(&outside))
            .await
            .expect("configured");
        assert_eq!(absolute, outside.canonicalize().expect("canonical"));
        let fallback = configured(Some(&root), None).await.expect("root");
        assert_eq!(fallback, root.canonicalize().expect("canonical"));
        assert!(
            configured(Some(&root), Some(Path::new("missing")))
                .await
                .is_err()
        );
    }
}
