use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::path::PathBuf;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use tokio::io::AsyncWriteExt as _;

use crate::store_path::single_component;

const DIR: &str = "claude-pending";
const DIR_MODE: u32 = 0o700;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pending {
    pub(crate) key: String,
    pub(crate) prompt: String,
    pub(crate) at: DateTime<Utc>,
}

#[derive(Debug)]
pub(crate) struct PendingPrompts {
    dir: PathBuf,
}

impl PendingPrompts {
    pub(crate) fn new(state_dir: &Path) -> Self {
        Self {
            dir: state_dir.join(DIR),
        }
    }

    fn file(&self, native: &str) -> Option<PathBuf> {
        single_component(native).then(|| self.dir.join(format!("{native}.jsonl")))
    }

    pub(crate) async fn add(&self, native: &str, pending: &Pending) -> std::io::Result<()> {
        let Some(file) = self.file(native) else {
            return Ok(());
        };
        tokio::fs::create_dir_all(&self.dir).await?;
        tokio::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(DIR_MODE)).await?;
        let mut line = serde_json::to_string(pending)?;
        line.push('\n');
        let mut handle = tokio::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(file)
            .await?;
        handle.write_all(line.as_bytes()).await?;
        handle.flush().await
    }

    pub(crate) async fn take(&self, native: &str) -> std::io::Result<Vec<Pending>> {
        let Some(file) = self.file(native) else {
            return Ok(Vec::new());
        };
        if !tokio::fs::try_exists(&file).await? {
            return Ok(Vec::new());
        }
        let text = tokio::fs::read_to_string(&file).await?;
        let taken = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str::<Pending>)
            .collect::<Result<Vec<_>, _>>()?;
        tokio::fs::remove_file(&file).await?;
        Ok(taken)
    }
}
