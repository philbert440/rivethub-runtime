use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, SubsecRound as _, TimeDelta, Utc};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use wire::mesh::{MeshFile, MeshNode, MeshNodeStatus, MeshParseCode};

use super::storage::Storage;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("mesh registry I/O failed")]
    Io(#[from] std::io::Error),
    #[error("mesh registry JSON is invalid")]
    Json(#[from] serde_json::Error),
    #[error("mesh registry uses the unsupported flat nodes array format")]
    FlatArray,
    #[error("mesh registry version {0} is unsupported")]
    Version(u32),
    #[error("mesh node key {0} does not match a nonempty node id")]
    NodeId(String),
    #[error("mesh path escapes its configured root: {}", .0.display())]
    OutsideRoot(PathBuf),
    #[error("mesh registry is locked by another writer")]
    Busy,
    #[error("mesh registry changed during the transaction")]
    Conflict,
    #[error("mesh registry operation was cancelled")]
    Cancelled,
    #[error("mesh registry worker failed")]
    Worker(#[from] tokio::task::JoinError),
    #[error("stale duration must be positive")]
    StaleDuration,
    #[error("heartbeat interval must be positive and shorter than the stale duration")]
    HeartbeatInterval,
}

impl RegistryError {
    pub fn parse_code(&self) -> Option<MeshParseCode> {
        match self {
            Self::Json(error) if error.is_syntax() || error.is_eof() => {
                Some(MeshParseCode::JsonInvalid)
            }
            Self::Json(_) | Self::Version(_) | Self::NodeId(_) => Some(MeshParseCode::InvalidShape),
            Self::FlatArray => Some(MeshParseCode::FlatArray),
            Self::Io(_)
            | Self::OutsideRoot(_)
            | Self::Busy
            | Self::Conflict
            | Self::Cancelled
            | Self::Worker(_)
            | Self::StaleDuration
            | Self::HeartbeatInterval => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Registry {
    storage: Arc<Storage>,
}

impl Registry {
    pub async fn open(root: PathBuf, relative: PathBuf) -> Result<Self, RegistryError> {
        let storage =
            tokio::task::spawn_blocking(move || Storage::open(&root, &relative)).await??;
        Ok(Self {
            storage: Arc::new(storage),
        })
    }

    pub async fn snapshot(&self) -> Result<Option<MeshFile>, RegistryError> {
        let storage = Arc::clone(&self.storage);
        tokio::task::spawn_blocking(move || storage.read()).await?
    }

    pub async fn register(
        &self,
        node: MeshNode,
        now: DateTime<Utc>,
        shutdown: CancellationToken,
    ) -> Result<MeshFile, RegistryError> {
        self.change(now, shutdown, move |file| {
            register(file, node, now);
            Ok(())
        })
        .await
    }

    pub async fn heartbeat(
        &self,
        local: MeshNode,
        now: DateTime<Utc>,
        stale_after: TimeDelta,
        shutdown: CancellationToken,
    ) -> Result<MeshFile, RegistryError> {
        if stale_after <= TimeDelta::zero() {
            return Err(RegistryError::StaleDuration);
        }
        let now = now.trunc_subsecs(3);
        self.change(now, shutdown, move |file| {
            if let Some(node) = file.nodes.get_mut(&local.id) {
                node.last_seen = now.max(node.last_seen);
            } else {
                register(file, local.clone(), now);
            }
            file.nodes.retain(|id, node| {
                *id == local.id || now.signed_duration_since(node.last_seen) <= stale_after
            });
            Ok(())
        })
        .await
    }

    pub async fn unregister(
        &self,
        id: String,
        now: DateTime<Utc>,
        shutdown: CancellationToken,
    ) -> Result<MeshFile, RegistryError> {
        self.change(now, shutdown, move |file| {
            file.nodes.remove(&id);
            Ok(())
        })
        .await
    }

    async fn change(
        &self,
        now: DateTime<Utc>,
        shutdown: CancellationToken,
        edit: impl FnOnce(&mut MeshFile) -> Result<(), RegistryError> + Send + 'static,
    ) -> Result<MeshFile, RegistryError> {
        let storage = Arc::clone(&self.storage);
        tokio::task::spawn_blocking(move || storage.change(now, &shutdown, edit)).await?
    }
}

pub(super) fn validate(file: &MeshFile) -> Result<(), RegistryError> {
    if file.version != 1 {
        return Err(RegistryError::Version(file.version));
    }
    for (key, node) in &file.nodes {
        if node.id.trim().is_empty() || *key != node.id {
            return Err(RegistryError::NodeId(key.clone()));
        }
    }
    Ok(())
}

fn register(file: &mut MeshFile, mut node: MeshNode, now: DateTime<Utc>) {
    let now = now.trunc_subsecs(3);
    node.status = MeshNodeStatus::Online;
    node.last_seen = now;
    node.registered_at = now;
    if let Some(previous) = file.nodes.remove(&node.id) {
        node.registered_at = previous.registered_at;
        node.last_seen = now.max(previous.last_seen);
        node.ssh_user = previous.ssh_user.or(node.ssh_user);
        node.install_root = previous.install_root.or(node.install_root);
        node.platform = previous.platform.or(node.platform);
        node.metadata = previous.metadata.or(node.metadata);
        node.extra.extend(previous.extra);
    }
    file.nodes.insert(node.id.clone(), node);
}
