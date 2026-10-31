use std::io::ErrorKind;
use std::io::Write as _;
use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::Utc;
use kernel::hub_state::HubStateStore;
use kernel::hub_state::HubStateStoreError;
use kernel::hub_state::HubStateWrite;
use kernel::hub_state::Versioned;
use tempfile::NamedTempFile;
use tokio::sync::Mutex;
use wire::gateway::hub_state::HubState;
use wire::gateway::hub_state::HubStateResponse;

const DIRECTORY: &str = "hub-state";
const PLAIN_NAME: usize = 96;

#[derive(Debug)]
pub(crate) struct FileHubState {
    dir: PathBuf,
    writes: Mutex<()>,
}

impl FileHubState {
    pub(crate) fn new(state_dir: &Path) -> Self {
        Self {
            dir: state_dir.join(DIRECTORY),
            writes: Mutex::new(()),
        }
    }

    fn path(&self, user: &str) -> PathBuf {
        let plain = user.len() <= PLAIN_NAME
            && !user.starts_with('.')
            && user
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        let name = if plain {
            user.to_owned()
        } else {
            format!("x-{}", hex::encode(user))
        };
        self.dir.join(format!("{name}.json"))
    }

    async fn read(&self, user: &str) -> Result<HubStateResponse, HubStateStoreError> {
        let path = self.path(user);
        tokio::task::spawn_blocking(move || read_file(&path))
            .await
            .map_err(unavailable)?
    }
}

#[async_trait]
impl HubStateStore<HubState> for FileHubState {
    async fn load(&self, user: &str) -> Result<Versioned<HubState>, HubStateStoreError> {
        self.read(user).await.map(versioned)
    }

    async fn replace(
        &self,
        user: &str,
        base_version: u64,
        state: HubState,
    ) -> Result<HubStateWrite<HubState>, HubStateStoreError> {
        let _write = self.writes.lock().await;
        let current = self.read(user).await?;
        if current.version != base_version {
            return Ok(HubStateWrite::Conflict(versioned(current)));
        }
        let next = HubStateResponse {
            version: base_version + 1,
            state,
        };
        let path = self.path(user);
        let written = next.clone();
        tokio::task::spawn_blocking(move || write_file(&path, &written))
            .await
            .map_err(unavailable)??;
        Ok(HubStateWrite::Saved(versioned(next)))
    }
}

fn versioned(document: HubStateResponse) -> Versioned<HubState> {
    Versioned {
        version: document.version,
        state: document.state,
    }
}

fn read_file(path: &Path) -> Result<HubStateResponse, HubStateStoreError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(HubStateResponse::default());
        }
        Err(error) => return Err(unavailable(error)),
    };
    match serde_json::from_slice(&bytes) {
        Ok(document) => Ok(document),
        Err(error) => {
            let aside = path.with_extension(format!("corrupt-{}", Utc::now().timestamp_millis()));
            tracing::warn!(path = %path.display(), aside = %aside.display(), %error, "hub state file is unreadable; moved aside");
            std::fs::rename(path, &aside).map_err(unavailable)?;
            Ok(HubStateResponse::default())
        }
    }
}

fn write_file(path: &Path, document: &HubStateResponse) -> Result<(), HubStateStoreError> {
    let parent = path
        .parent()
        .ok_or_else(|| unavailable(std::io::Error::from(ErrorKind::InvalidInput)))?;
    std::fs::create_dir_all(parent).map_err(unavailable)?;
    let mut temporary = NamedTempFile::new_in(parent).map_err(unavailable)?;
    serde_json::to_writer_pretty(&mut temporary, document).map_err(unavailable)?;
    temporary.write_all(b"\n").map_err(unavailable)?;
    temporary.as_file().sync_all().map_err(unavailable)?;
    temporary
        .persist(path)
        .map_err(|error| unavailable(error.error))?;
    Ok(())
}

fn unavailable(error: impl std::error::Error + Send + Sync + 'static) -> HubStateStoreError {
    HubStateStoreError::Unavailable(Box::new(error))
}
