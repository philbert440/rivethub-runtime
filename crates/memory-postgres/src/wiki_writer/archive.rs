use std::path::Path;

use chrono::{DateTime, Utc};
use kernel::wiki::builder::WikiWriteError;
use kernel::wiki::page::WikiPage;
use kernel::wiki::page::bound::{
    ARCHIVE_DIRECTORY, Archived, archive_header, archive_path, bound_page, settle_archive,
};
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
    let inline = head(root).await;
    let Some(block) = settle_archive(page, at, inline.as_deref(), &mut archived) else {
        return Ok(Vec::new());
    };
    Ok(vec![append(root, page, &block).await?])
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

async fn append(root: &Path, page: &WikiPage, block: &str) -> Result<String, WikiWriteError> {
    let slug = page.meta.slug.as_ref();
    let directory = root.join(ARCHIVE_DIRECTORY);
    reject_symlink(&directory).await?;
    tokio::fs::create_dir_all(&directory).await?;
    let relative = archive_path(slug);
    let destination = root.join(&relative);
    reject_symlink(&destination).await?;
    let fresh = !tokio::fs::try_exists(&destination).await?;
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&destination)
        .await?;
    if fresh {
        file.write_all(archive_header(&page.meta.title, slug).as_bytes())
            .await?;
    }
    file.write_all(block.as_bytes()).await?;
    file.sync_all().await?;
    drop(file);
    git(root, &["add", "--", &relative]).await?;
    Ok(relative)
}
