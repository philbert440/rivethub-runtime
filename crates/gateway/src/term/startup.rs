use std::path::{Path, PathBuf};

use wire::harness::HarnessId;

use super::TerminalService;
use crate::error::ApiError;

const FILE_FLAG: &str = "--append-system-prompt-file";

impl TerminalService {
    #[must_use]
    pub fn with_startup_dir(mut self, directory: PathBuf) -> Self {
        self.startup_dir = Some(directory);
        self
    }

    pub(super) async fn startup_file(
        &self,
        harness: HarnessId,
        session: &str,
        inline: [String; 2],
    ) -> Result<[String; 2], ApiError> {
        let Some(path) = self.startup_path(harness, session) else {
            return Ok(inline);
        };
        let [_, text] = inline;
        write_private(&path, &text).await?;
        Ok([FILE_FLAG.to_owned(), path.to_string_lossy().into_owned()])
    }

    pub(super) async fn forget_startup(&self, session: &str) {
        let Some(path) = self.startup_path(HarnessId::ClaudeCode, session) else {
            return;
        };
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(%error, "a terminal's startup file was not removed"),
        }
    }

    fn startup_path(&self, harness: HarnessId, session: &str) -> Option<PathBuf> {
        let directory = self.startup_dir.as_ref()?;
        let named = !session.is_empty()
            && session
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        (harness == HarnessId::ClaudeCode && named).then(|| directory.join(format!("{session}.md")))
    }
}

async fn write_private(path: &Path, text: &str) -> Result<(), ApiError> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncWriteExt;
    if let Some(directory) = path.parent() {
        tokio::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .await
            .map_err(ApiError::Io)?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .await
        .map_err(ApiError::Io)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .await
        .map_err(ApiError::Io)?;
    file.write_all(text.as_bytes())
        .await
        .map_err(ApiError::Io)?;
    file.flush().await.map_err(ApiError::Io)
}
