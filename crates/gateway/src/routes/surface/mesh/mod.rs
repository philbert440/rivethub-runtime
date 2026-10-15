use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use axum::Json;
use axum::extract::State;
use parking_lot::Mutex;
use tokio::task::JoinSet;
use wire::gateway::HealthzResponse;
use wire::gateway::devices::HarnessCapability;
use wire::gateway::devices::HarnessDriveCapability;
use wire::gateway::devices::HarnessInspectCapability;
use wire::gateway::devices::MeshOverview;
use wire::gateway::devices::MeshOverviewNode;
use wire::gateway::devices::NodeCapabilities;
use wire::gateway::devices::Subsystems;
use wire::harness::HarnessId;
use wire::mesh::MeshFile;
use wire::mesh::MeshParseCode;

use crate::Gateway;
use crate::Settings;
use crate::error::ApiError;
use crate::mesh::Registry;
use crate::mesh::RegistryError;

mod project;

const HARNESSES: &[HarnessId] = &[
    HarnessId::ClaudeCode,
    HarnessId::GrokBuild,
    HarnessId::Hermes,
    HarnessId::KimiCode,
    HarnessId::Pi,
    HarnessId::QwenCode,
    HarnessId::Cursor,
    HarnessId::Cowork,
    HarnessId::Opencode,
    HarnessId::Codex,
];

#[derive(Debug, Default)]
pub(crate) struct Cache {
    slot: Mutex<Option<Slot>>,
}

#[derive(Debug, Clone)]
struct Slot {
    at: Instant,
    overview: MeshOverview,
}

enum Read {
    File(MeshFile),
    Absent,
    Unreadable,
    Shape(MeshParseCode),
}

impl Cache {
    fn lookup(&self, now: Instant, ttl: Duration) -> Option<MeshOverview> {
        let slot = self.slot.lock();
        let overview = slot
            .as_ref()
            .and_then(|stored| fresh(stored.at, now, ttl).then(|| stored.overview.clone()));
        drop(slot);
        overview
    }

    fn store(&self, now: Instant, ttl: Duration, overview: MeshOverview) {
        if ttl.is_zero() {
            return;
        }
        *self.slot.lock() = Some(Slot { at: now, overview });
    }
}

fn fresh(stored_at: Instant, now: Instant, ttl: Duration) -> bool {
    !ttl.is_zero() && now.saturating_duration_since(stored_at) < ttl
}

pub(super) async fn overview(
    State(gateway): State<Arc<Gateway>>,
) -> Result<Json<MeshOverview>, ApiError> {
    let now = Instant::now();
    let ttl = gateway.settings().mesh_cache_ttl;
    if let Some(cached) = gateway.mesh_cache().lookup(now, ttl) {
        return Ok(Json(cached));
    }
    let built = load_overview(gateway.settings()).await?;
    gateway
        .mesh_cache()
        .store(Instant::now(), ttl, built.clone());
    Ok(Json(built))
}

pub(super) async fn capabilities(
    State(gateway): State<Arc<Gateway>>,
) -> Result<Json<NodeCapabilities>, ApiError> {
    let mesh = mesh_present(gateway.settings()).await?;
    Ok(Json(NodeCapabilities {
        subsystems: Subsystems {
            gateway: true,
            devices: gateway.settings().devices_enabled,
            mesh,
        },
        harnesses: harnesses(),
    }))
}

async fn load_overview(settings: &Settings) -> Result<MeshOverview, ApiError> {
    for path in candidates(settings) {
        match read_registry(&path).await? {
            Read::File(file) => {
                let projected = project::project(&file);
                return activate(settings, projected.overview).await;
            }
            Read::Absent | Read::Unreadable => {}
            Read::Shape(code) => {
                tracing::error!("mesh.json parse failed: {code}");
                return Err(ApiError::MeshParse(code));
            }
        }
    }
    Err(ApiError::NotFound("no mesh file"))
}

fn candidates(settings: &Settings) -> Vec<PathBuf> {
    if let Some(path) = &settings.mesh_file {
        return vec![path.clone()];
    }
    vec![
        settings.shared_dir.join("mesh.json"),
        settings.legacy_mesh_dir.join("mesh.json"),
    ]
}

async fn read_registry(path: &Path) -> Result<Read, ApiError> {
    let Some((parent, name)) = mesh_location(path) else {
        tracing::error!(path = %path.display(), "mesh path has no parent directory");
        return Err(ApiError::Internal);
    };
    let opened = match Registry::open(parent, name).await {
        Ok(registry) => registry.snapshot().await,
        Err(error) => Err(error),
    };
    match opened {
        Ok(Some(file)) => Ok(Read::File(file)),
        Ok(None) => Ok(Read::Absent),
        Err(error) => classify(error),
    }
}

fn mesh_location(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())?;
    let name = path.file_name()?;
    Some((parent.to_path_buf(), PathBuf::from(name)))
}

fn classify(error: RegistryError) -> Result<Read, ApiError> {
    if let Some(code) = error.parse_code() {
        return match code {
            MeshParseCode::JsonInvalid => Ok(Read::Unreadable),
            MeshParseCode::InvalidShape | MeshParseCode::FlatArray => Ok(Read::Shape(code)),
        };
    }
    let rendered = error.to_string();
    match error {
        RegistryError::Io(io) if io.kind() == ErrorKind::NotFound => Ok(Read::Absent),
        RegistryError::Io(io) => Err(io.into()),
        RegistryError::Worker(error) => {
            tracing::error!("mesh registry worker: {error}");
            Err(ApiError::Task(error))
        }
        RegistryError::Json(_)
        | RegistryError::OutsideRoot(_)
        | RegistryError::FlatArray
        | RegistryError::Version(_)
        | RegistryError::NodeId(_)
        | RegistryError::Busy
        | RegistryError::Conflict
        | RegistryError::Cancelled
        | RegistryError::StaleDuration
        | RegistryError::HeartbeatInterval => {
            tracing::error!("mesh registry: {rendered}");
            Err(ApiError::Internal)
        }
    }
}

async fn activate(
    settings: &Settings,
    mut overview: MeshOverview,
) -> Result<MeshOverview, ApiError> {
    let local = settings.node.clone();
    for node in &mut overview.nodes {
        if node.id == local {
            node.sessions = Some(0);
        }
    }
    probe_nodes(settings, &mut overview.nodes).await?;
    order_nodes(&mut overview.nodes, &local);
    Ok(overview)
}

fn order_nodes(nodes: &mut [MeshOverviewNode], local: &str) {
    nodes.sort_by(|left, right| {
        let left_local = left.id == local;
        let right_local = right.id == local;
        right_local
            .cmp(&left_local)
            .then_with(|| right.id.cmp(&left.id))
    });
}

async fn probe_nodes(settings: &Settings, nodes: &mut [MeshOverviewNode]) -> Result<(), ApiError> {
    let targets: Vec<(usize, String)> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (index, node.url.clone()))
        .collect();
    if targets.is_empty() {
        return Ok(());
    }
    let roots = match &settings.mesh_ca {
        Some(path) => Some(mesh_roots(path).await?),
        None => None,
    };
    let client = probe_client(settings.probe_timeout, roots)?;
    let mut tasks = JoinSet::new();
    for (index, url) in targets {
        let client = client.clone();
        tasks.spawn(async move { (index, probe_online(&client, &url).await) });
    }
    while let Some(joined) = tasks.join_next().await {
        let (index, online) = match joined {
            Ok(pair) => pair,
            Err(error) => {
                tracing::error!("mesh probe task failed: {error}");
                return Err(ApiError::Task(error));
            }
        };
        if let Some(node) = nodes.get_mut(index) {
            node.online = online;
        }
    }
    Ok(())
}

async fn mesh_roots(path: &std::path::Path) -> Result<Vec<reqwest::Certificate>, ApiError> {
    let pem = tokio::fs::read(path).await.map_err(|error| {
        tracing::error!("mesh CA bundle {}: {error}", path.display());
        ApiError::Internal
    })?;
    reqwest::Certificate::from_pem_bundle(&pem).map_err(|error| {
        tracing::error!("mesh CA bundle {}: {error}", path.display());
        ApiError::Internal
    })
}

fn probe_client(
    timeout: Duration,
    roots: Option<Vec<reqwest::Certificate>>,
) -> Result<reqwest::Client, ApiError> {
    let builder = match roots {
        Some(roots) => reqwest::Client::builder().tls_certs_only(roots),
        None => reqwest::Client::builder(),
    };
    builder
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| {
            tracing::error!("mesh probe client: {error}");
            ApiError::Internal
        })
}

async fn probe_online(client: &reqwest::Client, url: &str) -> bool {
    let target = format!("{}/healthz", url.trim_end_matches('/'));
    let response = match client.get(target).send().await {
        Ok(response) => response,
        Err(error) => {
            tracing::debug!("mesh probe: {error}");
            return false;
        }
    };
    if !response.status().is_success() {
        tracing::debug!(status = %response.status(), "mesh probe status");
        return false;
    }
    match response.json::<HealthzResponse>().await {
        Ok(_) => true,
        Err(error) => {
            tracing::debug!("mesh healthz: {error}");
            false
        }
    }
}

async fn mesh_present(settings: &Settings) -> Result<bool, ApiError> {
    let paths = candidates(settings);
    let joined = tokio::task::spawn_blocking(move || paths.iter().any(|path| path.is_file())).await;
    super::super::join_task(joined)
}

fn harnesses() -> Vec<HarnessCapability> {
    HARNESSES.iter().copied().map(harness_capability).collect()
}

fn harness_capability(id: HarnessId) -> HarnessCapability {
    HarnessCapability {
        harness_id: id,
        inspect: HarnessInspectCapability {
            list: true,
            read: true,
        },
        drive: HarnessDriveCapability {
            drive: id != HarnessId::Cowork,
            resume: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::Read;
    use super::fresh;
    use super::load_overview;
    use super::order_nodes;
    use super::project::project;
    use super::read_registry;
    use crate::Settings;
    use chrono::DateTime;
    use serde::Serialize;
    use std::path::PathBuf;
    use std::time::Duration;
    use std::time::Instant;
    use wire::Extra;
    use wire::mesh::MeshFile;
    use wire::mesh::MeshNode;
    use wire::mesh::MeshNodeStatus;
    use wire::mesh::MeshParseCode;

    fn settings_for(mesh_file: Option<PathBuf>, shared: PathBuf, legacy: PathBuf) -> Settings {
        Settings {
            hostname: "h".to_owned(),
            node: "n".to_owned(),
            allowed_origins: Vec::new(),
            allowed_hosts: Vec::new(),
            files_root: None,
            static_dir: None,
            root_redirect: None,
            state_dir: shared.clone(),
            directory_root: shared.join("agents"),
            mesh_file,
            shared_dir: shared,
            legacy_mesh_dir: legacy,
            allowed_harnesses: None,
            devices_enabled: false,
            probe_timeout: Duration::from_millis(50),
            mesh_cache_ttl: Duration::ZERO,
            mesh_ca: None,
        }
    }

    #[derive(Serialize)]
    struct Flat {
        nodes: Vec<MeshNode>,
    }

    fn stored_node(id: &str) -> MeshNode {
        MeshNode {
            id: id.to_owned(),
            name: id.to_owned(),
            role: None,
            agents: Vec::new(),
            host: "127.0.0.1".to_owned(),
            port: 1,
            providers: Vec::new(),
            models: Vec::new(),
            capabilities: vec!["den".to_owned()],
            status: MeshNodeStatus::Offline,
            last_seen: DateTime::UNIX_EPOCH,
            registered_at: DateTime::UNIX_EPOCH,
            version: "0".to_owned(),
            metadata: None,
            ssh_user: None,
            install_root: None,
            platform: None,
            extra: Extra::default(),
        }
    }

    #[test]
    fn node_order_is_local_then_id_descending() {
        let mut document = MeshFile::empty(DateTime::UNIX_EPOCH);
        for (id, name, host) in [
            ("golden-node", "Golden Node", "127.0.0.1"),
            ("ported", "ported", "ported.example"),
            ("peer-refused", "Peer Refused", "127.0.0.1"),
            ("peer-https", "peer-https", "peer-https.invalid"),
            ("peer-cap", "peer-cap", "peer-cap.invalid"),
            ("bad-port", "bad", "bad.example"),
        ] {
            let mut node = stored_node(id);
            node.name = name.to_owned();
            node.host = host.to_owned();
            document.nodes.insert(id.to_owned(), node);
        }
        let mut overview = project(&document).overview;
        order_nodes(&mut overview.nodes, "golden-node");
        let ids: Vec<&str> = overview.nodes.iter().map(|node| node.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "golden-node",
                "ported",
                "peer-refused",
                "peer-https",
                "peer-cap",
                "bad-port"
            ]
        );
    }

    #[test]
    fn cache_freshness_is_a_pure_comparison() {
        let start = Instant::now();
        assert!(!fresh(start, start, Duration::ZERO));
        assert!(fresh(start, start, Duration::from_secs(10)));
        assert!(fresh(
            start,
            start + Duration::from_secs(9),
            Duration::from_secs(10)
        ));
        assert!(!fresh(
            start,
            start + Duration::from_secs(10),
            Duration::from_secs(10)
        ));
        assert!(fresh(
            start + Duration::from_secs(5),
            start,
            Duration::from_secs(10)
        ));
    }

    #[tokio::test]
    async fn shape_errors_do_not_fall_through_and_truncation_does()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let shared = dir.path().join("shared");
        let legacy = dir.path().join("legacy");
        std::fs::create_dir(&shared)?;
        std::fs::create_dir(&legacy)?;
        let good = MeshFile::empty(DateTime::UNIX_EPOCH);
        std::fs::write(legacy.join("mesh.json"), serde_json::to_vec(&good)?)?;
        std::fs::write(shared.join("mesh.json"), b"{")?;
        let overview = load_overview(&settings_for(None, shared.clone(), legacy.clone())).await?;
        assert_eq!(overview.nodes.len(), 0);
        std::fs::write(shared.join("mesh.json"), br#"{"nodes":1}"#)?;
        let shape = load_overview(&settings_for(None, shared.clone(), legacy.clone())).await;
        assert!(matches!(
            shape,
            Err(crate::error::ApiError::MeshParse(
                MeshParseCode::InvalidShape
            ))
        ));
        let flat_path = dir.path().join("flat.json");
        std::fs::write(
            &flat_path,
            serde_json::to_vec(&Flat {
                nodes: vec![stored_node("x")],
            })?,
        )?;
        assert!(matches!(
            read_registry(&flat_path).await?,
            Read::Shape(MeshParseCode::FlatArray)
        ));
        let array = dir.path().join("array.json");
        std::fs::write(&array, br#"[{"id":"x"}]"#)?;
        assert!(matches!(
            read_registry(&array).await?,
            Read::Shape(MeshParseCode::InvalidShape)
        ));
        let empty = dir.path().join("empty.json");
        std::fs::write(&empty, b"{}")?;
        assert!(matches!(
            read_registry(&empty).await?,
            Read::Shape(MeshParseCode::InvalidShape)
        ));
        let missing = dir.path().join("absent.json");
        assert!(matches!(read_registry(&missing).await?, Read::Absent));
        let gone = dir.path().join("missing-parent").join("mesh.json");
        assert!(matches!(read_registry(&gone).await?, Read::Absent));
        Ok(())
    }
}
