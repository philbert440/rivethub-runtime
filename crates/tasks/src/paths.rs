use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use crate::error::Error;

pub(crate) async fn contained(root: &Path, working: &Path) -> Result<PathBuf, Error> {
    if root.as_os_str().is_empty() || working.as_os_str().is_empty() {
        return Err(Error::FilesRootRequired);
    }
    if working
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(Error::PathEscape);
    }
    let joined = if working.is_absolute() {
        working.to_path_buf()
    } else {
        root.join(working)
    };
    let canonical = lookup(joined).await?;
    let root_canonical = lookup(root.to_path_buf()).await?;
    if canonical.starts_with(&root_canonical) {
        Ok(canonical)
    } else {
        Err(Error::PathEscape)
    }
}

pub(crate) async fn configured(root: Option<&Path>, working: &Path) -> Result<PathBuf, Error> {
    if working.is_absolute() {
        return lookup(working.to_path_buf()).await;
    }
    contained(root.ok_or(Error::FilesRootRequired)?, working).await
}

async fn lookup(path: PathBuf) -> Result<PathBuf, Error> {
    let found = tokio::task::spawn_blocking(move || std::fs::canonicalize(path))
        .await
        .map_err(Error::Blocking)?;
    match found {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Error::PathMissing),
        Err(error) => Err(Error::PathIo(error)),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::contained;
    use crate::error::Error;

    #[tokio::test]
    async fn a_relative_path_inside_the_root_is_kept() {
        let root = tempfile::tempdir().expect("root");
        let work = root.path().join("work");
        std::fs::create_dir(&work).expect("work");
        let found = contained(root.path(), Path::new("work"))
            .await
            .expect("contained");
        assert_eq!(found, work.canonicalize().expect("canonical"));
    }

    #[tokio::test]
    async fn an_absolute_path_inside_the_root_is_kept() {
        let root = tempfile::tempdir().expect("root");
        let work = root.path().join("work");
        std::fs::create_dir(&work).expect("work");
        let found = contained(root.path(), &work).await.expect("contained");
        assert_eq!(found, work.canonicalize().expect("canonical"));
    }

    #[tokio::test]
    async fn a_parent_segment_is_refused_before_it_is_resolved() {
        let root = tempfile::tempdir().expect("root");
        std::fs::create_dir(root.path().join("nested")).expect("nested");
        let error = contained(root.path(), Path::new("nested/.."))
            .await
            .expect_err("escape");
        assert!(matches!(error, Error::PathEscape));
    }

    #[tokio::test]
    async fn a_sibling_prefix_is_not_inside_the_root() {
        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("files");
        let sibling = parent.path().join("files-extra");
        std::fs::create_dir(&root).expect("root");
        std::fs::create_dir(&sibling).expect("sibling");
        let error = contained(&root, &sibling).await.expect_err("escape");
        assert!(matches!(error, Error::PathEscape));
    }

    #[tokio::test]
    async fn a_symlink_that_leaves_the_root_is_refused() {
        let parent = tempfile::tempdir().expect("parent");
        let root = parent.path().join("files");
        let outside = parent.path().join("outside");
        std::fs::create_dir(&root).expect("root");
        std::fs::create_dir(&outside).expect("outside");
        std::os::unix::fs::symlink(&outside, root.join("link")).expect("link");
        let error = contained(&root, Path::new("link"))
            .await
            .expect_err("escape");
        assert!(matches!(error, Error::PathEscape));
    }

    #[tokio::test]
    async fn a_symlink_that_stays_inside_the_root_is_kept() {
        let root = tempfile::tempdir().expect("root");
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).expect("nested");
        std::os::unix::fs::symlink(&nested, root.path().join("link")).expect("link");
        let found = contained(root.path(), Path::new("link"))
            .await
            .expect("contained");
        assert_eq!(found, nested.canonicalize().expect("canonical"));
    }

    #[tokio::test]
    async fn a_missing_path_is_not_an_escape() {
        let root = tempfile::tempdir().expect("root");
        let error = contained(root.path(), Path::new("missing"))
            .await
            .expect_err("missing");
        assert!(matches!(error, Error::PathMissing));
    }

    #[tokio::test]
    async fn an_empty_path_is_refused() {
        let root = tempfile::tempdir().expect("root");
        let error = contained(root.path(), Path::new(""))
            .await
            .expect_err("empty");
        assert!(matches!(error, Error::FilesRootRequired));
        let error = contained(Path::new(""), root.path())
            .await
            .expect_err("empty root");
        assert!(matches!(error, Error::FilesRootRequired));
    }
}
