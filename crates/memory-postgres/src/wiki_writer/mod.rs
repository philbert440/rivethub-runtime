mod lock;

use std::path::{Component, Path, PathBuf};

use kernel::wiki::builder::WikiWriteError;
use tokio::process::Command;
use wire::gateway::wiki::WikiSlug;
use wire::wiki::{
    PagePatch, WikiPage, apply_patch, normalize_slug, parse_wiki_page, serialize_wiki_page,
};

use lock::WriterLock;

#[derive(Debug, Clone)]
pub struct WikiRepository {
    root: PathBuf,
}

#[derive(Debug)]
pub struct AppliedPage {
    pub page: WikiPage,
    pub git_sha: String,
}

impl WikiRepository {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn read_unlocked(&self, slug: &WikiSlug) -> Result<Option<WikiPage>, WikiWriteError> {
        let root = tokio::fs::canonicalize(&self.root).await?;
        read_page(&root, slug).await
    }

    pub async fn lock(&self) -> Result<LockedRepository, WikiWriteError> {
        let root = prepare_root(&self.root).await?;
        let lock = WriterLock::acquire(&root).await?;
        Ok(LockedRepository { root, lock })
    }
}

#[derive(Debug)]
pub struct LockedRepository {
    root: PathBuf,
    lock: WriterLock,
}

impl LockedRepository {
    pub async fn apply(
        &mut self,
        patch: PagePatch,
        provenance: &str,
    ) -> Result<AppliedPage, WikiWriteError> {
        apply_locked(&self.root, patch, provenance).await
    }

    pub async fn read(&self, slug: &str) -> Result<Option<WikiPage>, WikiWriteError> {
        let slug = WikiSlug::try_from(normalize_slug(slug))?;
        ensure_repo(&self.root).await?;
        read_page(&self.root, &slug).await
    }

    pub async fn fold(
        &mut self,
        page: &WikiPage,
        removed: &[String],
        message: &str,
    ) -> Result<String, WikiWriteError> {
        fold_locked(&self.root, page, removed, message).await
    }

    pub async fn release(self) -> Result<(), WikiWriteError> {
        self.lock.release().await
    }
}

async fn prepare_root(root: &Path) -> Result<PathBuf, WikiWriteError> {
    if !root.is_absolute() || root.components().any(|part| part == Component::ParentDir) {
        return Err(WikiWriteError::Containment(root.to_path_buf()));
    }
    let mut ancestor = PathBuf::new();
    for component in root.components() {
        ancestor.push(component);
        reject_symlink(&ancestor).await?;
    }
    tokio::fs::create_dir_all(root).await?;
    Ok(tokio::fs::canonicalize(root).await?)
}

async fn reject_symlink(path: &Path) -> Result<(), WikiWriteError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err(WikiWriteError::Containment(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

async fn ensure_repo(root: &Path) -> Result<(), WikiWriteError> {
    let topics = root.join("topics");
    reject_symlink(&topics).await?;
    tokio::fs::create_dir_all(&topics).await?;
    let topics = tokio::fs::canonicalize(topics).await?;
    if !topics.starts_with(root) {
        return Err(WikiWriteError::Containment(topics));
    }
    let git_path = root.join(".git");
    reject_symlink(&git_path).await?;
    if tokio::fs::try_exists(&git_path).await? {
        if !tokio::fs::metadata(&git_path).await?.is_dir() {
            return Err(WikiWriteError::Containment(git_path));
        }
    } else {
        git(root, &["init", "-b", "main", "."]).await?;
    }
    reject_symlink(&git_path.join("config")).await?;
    let top = git(root, &["rev-parse", "--show-toplevel"]).await?;
    let top = tokio::fs::canonicalize(top.trim()).await?;
    if top != root {
        return Err(WikiWriteError::Containment(top));
    }
    git(root, &["config", "--local", "user.name", "RivetOS Wiki"]).await?;
    git(
        root,
        &["config", "--local", "user.email", "wiki@rivetos.dev"],
    )
    .await?;
    Ok(())
}

async fn apply_locked(
    root: &Path,
    patch: PagePatch,
    provenance: &str,
) -> Result<AppliedPage, WikiWriteError> {
    let slug = WikiSlug::try_from(normalize_slug(&patch.slug))?;
    sync(root).await?;
    let page = read_page(root, &slug).await?;
    let prefix: String = provenance.chars().take(8).collect();
    let message = format!(
        "wiki({}): {} from summary {prefix}\n\nProvenance: summary {provenance}\nPipeline: wiki-v7",
        slug.as_ref(),
        patch.action
    );
    let page = apply_patch(page, patch)?;
    let relative = store_page(root, &page).await?;
    let git_sha = commit(root, &[relative], &message).await?;
    Ok(AppliedPage { page, git_sha })
}

async fn fold_locked(
    root: &Path,
    page: &WikiPage,
    removed: &[String],
    message: &str,
) -> Result<String, WikiWriteError> {
    sync(root).await?;
    let mut paths = vec![store_page(root, page).await?];
    for slug in removed {
        let slug = WikiSlug::try_from(normalize_slug(slug))?;
        let relative = topic_path(&slug);
        reject_symlink(&root.join(&relative)).await?;
        git(
            root,
            &["rm", "-q", "--cached", "--ignore-unmatch", "--", &relative],
        )
        .await?;
        match tokio::fs::remove_file(root.join(&relative)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        paths.push(relative);
    }
    commit(root, &paths, message).await
}

async fn sync(root: &Path) -> Result<(), WikiWriteError> {
    ensure_repo(root).await?;
    if tokio::fs::try_exists(root.join(".git/refs/remotes")).await? {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "CONTRACT B11: wiki pull failure warns and local page application continues"
        )]
        let _: Result<String, WikiWriteError> = git(root, &["pull", "--rebase"])
            .await
            .inspect_err(|error| tracing::warn!(%error, "wiki pull failed; applying local patch"));
    }
    reject_symlink(&root.join("topics")).await
}

fn topic_path(slug: &WikiSlug) -> String {
    format!("topics/{}.md", slug.as_ref())
}

async fn read_page(root: &Path, slug: &WikiSlug) -> Result<Option<WikiPage>, WikiWriteError> {
    let destination = root.join(topic_path(slug));
    reject_symlink(&destination).await?;
    match tokio::fs::canonicalize(&destination).await {
        Ok(canonical) => {
            if !canonical.starts_with(root) {
                return Err(WikiWriteError::Containment(canonical));
            }
            let markdown = tokio::fs::read_to_string(canonical).await?;
            let page = parse_wiki_page(&markdown)?;
            if page.meta.slug != *slug {
                return Err(WikiWriteError::SlugMismatch);
            }
            Ok(Some(page))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

async fn store_page(root: &Path, page: &WikiPage) -> Result<String, WikiWriteError> {
    let relative = topic_path(&page.meta.slug);
    let destination = root.join(&relative);
    reject_symlink(&destination).await?;
    let markdown = serialize_wiki_page(page)?;
    let temporary = root
        .join("topics")
        .join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let written = write_page(&temporary, &destination, markdown.as_bytes()).await;
    if let Err(error) = written {
        match tokio::fs::remove_file(&temporary).await {
            Ok(()) => {}
            Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => {}
            Err(cleanup) => tracing::error!(%cleanup, "wiki temporary file cleanup failed"),
        }
        return Err(error);
    }
    git(root, &["add", "--", &relative]).await?;
    Ok(relative)
}

async fn commit(root: &Path, paths: &[String], message: &str) -> Result<String, WikiWriteError> {
    let mut changed: Vec<&str> = Vec::new();
    for path in paths {
        if !git(root, &["status", "--porcelain", "--", path])
            .await?
            .is_empty()
        {
            changed.push(path);
        }
    }
    if !changed.is_empty() {
        let mut args = vec!["commit", "--only", "-m", message, "--"];
        args.extend(changed);
        git(root, &args).await?;
    }
    Ok(git(root, &["rev-parse", "HEAD"]).await?.trim().to_owned())
}

async fn write_page(temporary: &Path, path: &Path, markdown: &[u8]) -> Result<(), WikiWriteError> {
    use tokio::io::AsyncWriteExt as _;
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)
        .await?;
    file.write_all(markdown).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(temporary, path).await?;
    Ok(())
}

async fn git(root: &Path, args: &[&str]) -> Result<String, WikiWriteError> {
    let output = Command::new("git")
        .current_dir(root)
        .arg("--literal-pathspecs")
        .args(args)
        .env_remove("GIT_AUTHOR_NAME")
        .env_remove("GIT_AUTHOR_EMAIL")
        .env_remove("GIT_COMMITTER_NAME")
        .env_remove("GIT_COMMITTER_EMAIL")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .kill_on_drop(true)
        .output()
        .await?;
    if !output.status.success() {
        return Err(WikiWriteError::Git {
            operation: args.join(" "),
            message: String::from_utf8(output.stderr)?,
        });
    }
    Ok(String::from_utf8(output.stdout)?)
}
