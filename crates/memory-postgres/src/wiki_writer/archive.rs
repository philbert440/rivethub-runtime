use std::path::Path;

use chrono::{DateTime, Utc};
use kernel::wiki::builder::WikiWriteError;
use kernel::wiki::page::WikiPage;
use kernel::wiki::page::archive::{
    ARCHIVE_DIRECTORY, archive_directory, archive_header, archive_month, archive_path,
    settle_archive, unarchived,
};
use kernel::wiki::page::bound::{Archived, bound_page};
use tokio::io::AsyncWriteExt as _;

use super::{git, reject_symlink};

pub(super) async fn bound(
    root: &Path,
    previous: Option<&WikiPage>,
    page: &mut WikiPage,
    at: DateTime<Utc>,
    extra: Vec<Archived>,
) -> Result<Vec<String>, WikiWriteError> {
    let mut archived = extra;
    archived.extend(bound_page(previous, page));
    if archived.is_empty() {
        return Ok(Vec::new());
    }
    let slug = page.meta.slug.as_ref().to_owned();
    let month = archive_month(at);
    let relative = archive_path(&slug, &month);
    let destination = root.join(&relative);
    reject_symlink(&root.join(ARCHIVE_DIRECTORY)).await?;
    reject_symlink(&root.join(archive_directory(&slug))).await?;
    reject_symlink(&destination).await?;
    let stored = match tokio::fs::read_to_string(&destination).await {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut archived = unarchived(archived, page, stored.as_deref().unwrap_or_default());
    let inline = head(root).await;
    let Some(block) = settle_archive(page, at, inline.as_deref(), &mut archived) else {
        return Ok(Vec::new());
    };
    let header = stored
        .is_none()
        .then(|| archive_header(&page.meta.title, &slug, &month));
    append(root, &relative, header.as_deref(), &block).await?;
    Ok(vec![relative])
}

async fn head(root: &Path) -> Option<String> {
    #[cfg_attr(
        dylint_lib = "rr_lints",
        expect(
            fallback,
            reason = "PATTERNS FALLBACK: a real absence, contract note 100: a repository without a commit has no commit to name"
        )
    )]
    let sha = git(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
        .await
        .ok();
    sha.map(|sha| sha.trim().to_owned())
        .filter(|sha| !sha.is_empty())
}

async fn append(
    root: &Path,
    relative: &str,
    header: Option<&str>,
    block: &str,
) -> Result<(), WikiWriteError> {
    let destination = root.join(relative);
    let Some(directory) = destination.parent() else {
        return Err(WikiWriteError::Containment(destination.clone()));
    };
    tokio::fs::create_dir_all(directory).await?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&destination)
        .await?;
    if let Some(header) = header {
        file.write_all(header.as_bytes()).await?;
    }
    file.write_all(block.as_bytes()).await?;
    file.sync_all().await?;
    drop(file);
    git(root, &["add", "--", relative]).await?;
    Ok(())
}
