use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use config::Environment;
use gateway::mesh::Registry;
use tokio_util::sync::CancellationToken;
use url::Url;
use wire::Extra;
use wire::mesh::{MeshFile, MeshMetadata, MeshNode, MeshNodeStatus, MeshRole};

use crate::Error;

pub(super) const GATEWAY_PORT: u16 = 5174;

pub(super) struct Draft {
    pub(super) name: String,
    pub(super) host: String,
    pub(super) agents: Vec<String>,
    pub(super) ssh_user: Option<String>,
}

pub(super) struct Saved {
    pub(super) warning: Option<String>,
}

pub(super) fn node(draft: Draft) -> Result<MeshNode, Error> {
    let port = GATEWAY_PORT;
    let gateway_url = Url::parse(&format!("http://{}:{port}", draft.host))?;
    let stamp = DateTime::<Utc>::UNIX_EPOCH;
    Ok(MeshNode {
        id: draft.name.clone(),
        name: draft.name,
        role: Some(MeshRole::Agent),
        agents: draft.agents,
        host: draft.host,
        port,
        providers: Vec::new(),
        models: Vec::new(),
        capabilities: Vec::new(),
        status: MeshNodeStatus::Online,
        last_seen: stamp,
        registered_at: stamp,
        version: env!("CARGO_PKG_VERSION").to_owned(),
        metadata: Some(MeshMetadata {
            gateway_url: Some(gateway_url),
            gateway_port: Some(port),
            extra: Extra::default(),
        }),
        ssh_user: draft.ssh_user,
        install_root: None,
        platform: None,
        extra: Extra::default(),
    })
}

pub(super) async fn mesh_path(env: &Environment) -> Result<PathBuf, Error> {
    if let Some(notice) = env.legacy_notice("RIVETHUB_MESH_FILE", "RIVETOS_DEN_MESH_FILE") {
        tracing::warn!("{notice}");
    }
    if let Some(path) = explicit_mesh(env) {
        return absolute_mesh(path);
    }
    let shared = config::shared_dir(env).join("mesh.json");
    let legacy = config::Paths::new(env)?.legacy_home().join("mesh.json");
    let chosen = tokio::task::spawn_blocking(move || choose_mesh(shared, legacy)).await??;
    absolute_mesh(chosen)
}

pub(super) async fn read_file(path: PathBuf) -> Result<MeshFile, Error> {
    let registry = open_existing(path).await?;
    match registry.snapshot().await? {
        Some(file) => Ok(file),
        None => Err(Error::NoMesh),
    }
}

pub(super) async fn open_writable(path: PathBuf) -> Result<Registry, Error> {
    let parent = parent_dir(&path)?;
    tokio::task::spawn_blocking(move || std::fs::create_dir_all(parent)).await??;
    let (parent, name) = location(&path)?;
    Ok(Registry::open(parent, name).await?)
}

pub(super) async fn save(
    registry: &Registry,
    node: MeshNode,
    shutdown: &CancellationToken,
) -> Result<Saved, Error> {
    let previous = registry.snapshot().await?;
    let warning = previous.as_ref().and_then(|file| host_warning(file, &node));
    drop(
        registry
            .register(node, Utc::now(), shutdown.clone())
            .await?,
    );
    Ok(Saved { warning })
}

pub(super) async fn forget(
    path: PathBuf,
    id: &str,
    shutdown: &CancellationToken,
) -> Result<(), Error> {
    let registry = open_existing(path).await?;
    let Some(file) = registry.snapshot().await? else {
        return Err(Error::NoMesh);
    };
    if !file.nodes.contains_key(id) {
        return Err(Error::UnknownMeshNode { id: id.to_owned() });
    }
    drop(
        registry
            .unregister(id.to_owned(), Utc::now(), shutdown.clone())
            .await?,
    );
    Ok(())
}

pub(super) fn list_text(file: &MeshFile) -> Result<String, Error> {
    let mut lines = String::new();
    for node in file.nodes.values() {
        let status = status_label(node.status)?;
        lines.push_str(&node.id);
        lines.push(' ');
        lines.push_str(&node.name);
        lines.push(' ');
        lines.push_str(&node.host);
        lines.push(' ');
        lines.push_str(&node.port.to_string());
        lines.push(' ');
        lines.push_str(&status);
        lines.push('\n');
    }
    Ok(lines)
}

pub(super) fn status_text(path: &Path, file: &MeshFile) -> Result<String, Error> {
    let mut lines = format!(
        "mesh {}\nupdated {}\n",
        path.display(),
        file.updated_at.to_rfc3339()
    );
    lines.push_str(&list_text(file)?);
    Ok(lines)
}

pub(super) fn file_json(file: &MeshFile) -> Result<String, Error> {
    Ok(format!("{}\n", serde_json::to_string(file)?))
}

fn explicit_mesh(env: &Environment) -> Option<PathBuf> {
    if let Some(value) = env.text("RIVETHUB_MESH_FILE") {
        return Some(PathBuf::from(value));
    }
    env.text("RIVETOS_DEN_MESH_FILE").map(PathBuf::from)
}

fn choose_mesh(shared: PathBuf, legacy: PathBuf) -> Result<PathBuf, std::io::Error> {
    if regular_file(&shared)? {
        Ok(shared)
    } else if regular_file(&legacy)? {
        Ok(legacy)
    } else {
        Ok(shared)
    }
}

fn regular_file(path: &Path) -> Result<bool, std::io::Error> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_file() || metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn absolute_mesh(path: PathBuf) -> Result<PathBuf, Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let named = path.file_name().is_some();
    let traversal = path
        .components()
        .any(|component| component == Component::ParentDir);
    if path.is_absolute() && parent.is_some() && named && !traversal {
        Ok(path)
    } else {
        Err(Error::MeshPath)
    }
}

async fn open_existing(path: PathBuf) -> Result<Registry, Error> {
    let present = tokio::task::spawn_blocking({
        let path = path.clone();
        move || regular_file(&path)
    })
    .await??;
    if !present {
        return Err(Error::NoMesh);
    }
    let (parent, name) = location(&path)?;
    Ok(Registry::open(parent, name).await?)
}

fn parent_dir(path: &Path) -> Result<PathBuf, Error> {
    Ok(location(path)?.0)
}

fn location(path: &Path) -> Result<(PathBuf, PathBuf), Error> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or(Error::MeshPath)?;
    let name = path.file_name().ok_or(Error::MeshPath)?;
    Ok((parent.to_path_buf(), PathBuf::from(name)))
}

fn host_warning(file: &MeshFile, node: &MeshNode) -> Option<String> {
    let previous = file.nodes.get(&node.id)?;
    if previous.host == node.host {
        return None;
    }
    let fields = kept_fields(previous, node);
    if fields.is_empty() {
        None
    } else {
        Some(format!(
            "Node {}: keeping {} recorded for host {} on new host {} — check mesh.json",
            node.id,
            fields.join(", "),
            previous.host,
            node.host
        ))
    }
}

fn kept_fields(previous: &MeshNode, node: &MeshNode) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if previous.ssh_user.is_some() && node.ssh_user.is_none() {
        fields.push("sshUser");
    }
    if previous.install_root.is_some() && node.install_root.is_none() {
        fields.push("installRoot");
    }
    if previous.platform.is_some() && node.platform.is_none() {
        fields.push("platform");
    }
    fields
}

fn status_label(status: MeshNodeStatus) -> Result<String, Error> {
    let encoded = serde_json::to_string(&status)?;
    Ok(serde_json::from_str(&encoded)?)
}
