use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use gateway::mesh::{Heartbeat, HeartbeatReport, Registry, RegistryError};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use wire::Extra;
use wire::mesh::{MeshFile, MeshMetadata, MeshNode, MeshNodeStatus, MeshRole};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn node(id: &str) -> MeshNode {
    MeshNode {
        id: id.to_owned(),
        name: id.to_owned(),
        role: Some(MeshRole::Agent),
        agents: vec!["assistant".to_owned()],
        host: "127.0.0.1".to_owned(),
        port: 5700,
        providers: vec!["inference".to_owned()],
        models: vec!["model".to_owned()],
        capabilities: vec!["chat".to_owned()],
        status: MeshNodeStatus::Online,
        last_seen: DateTime::UNIX_EPOCH,
        registered_at: DateTime::UNIX_EPOCH,
        version: "0.1.0".to_owned(),
        metadata: None,
        ssh_user: None,
        install_root: None,
        platform: None,
        extra: Extra::default(),
    }
}

async fn registry(root: &TempDir) -> Result<Registry, RegistryError> {
    Registry::open(root.path().to_owned(), PathBuf::from("mesh.json")).await
}

#[tokio::test]
async fn registration_and_external_operator_edits_survive_restart() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = registry(&root).await?;
    assert_eq!(registry.snapshot().await?, None);
    let now = DateTime::UNIX_EPOCH + TimeDelta::seconds(100);
    let initial = registry
        .register(node("local"), now, CancellationToken::new())
        .await?;
    let mut external = initial.clone();
    external.extra = serde_json::from_str(r#"{"operatorLabel":"fleet"}"#)?;
    let edited = external.nodes.get_mut("local").ok_or("local missing")?;
    edited.ssh_user = Some("operator".to_owned());
    edited.install_root = Some("/srv/RivetHub install".to_owned());
    edited.platform = Some("linux".to_owned());
    edited.metadata = Some(serde_json::from_str::<MeshMetadata>(r#"{"rack":"west"}"#)?);
    edited.extra = serde_json::from_str(r#"{"gatewayPort":5174,"futureField":{"a":1}}"#)?;
    external.nodes.insert("peer".to_owned(), node("peer"));
    std::fs::write(
        root.path().join("mesh.json"),
        serde_json::to_vec(&external)?,
    )?;
    let mut update = node("local");
    update.host = "node.example.test".to_owned();
    update.port = 5800;
    update.agents.push("reviewer".to_owned());
    update.ssh_user = Some("runtime-value".to_owned());
    update.extra = serde_json::from_str(r#"{"newField":true,"gatewayPort":9999}"#)?;
    let next = now + TimeDelta::seconds(10);
    let actual = registry
        .register(update.clone(), next, CancellationToken::new())
        .await?;
    let saved = external.nodes.get("local").ok_or("local missing")?;
    update.last_seen = next;
    update.registered_at = now;
    update.ssh_user.clone_from(&saved.ssh_user);
    update.install_root.clone_from(&saved.install_root);
    update.platform.clone_from(&saved.platform);
    update.metadata.clone_from(&saved.metadata);
    update.extra.extend(saved.extra.clone());
    external.nodes.insert("local".to_owned(), update);
    external.updated_at = next;
    assert_eq!(actual, external);
    assert_eq!(registry.snapshot().await?, Some(external));
    let bytes = std::fs::read(root.path().join("mesh.json"))?;
    assert_eq!(serde_json::from_slice::<MeshFile>(&bytes)?, actual);
    Ok(())
}

#[tokio::test]
async fn heartbeat_refreshes_self_prunes_only_expired_peers_and_preserves_status() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = registry(&root).await?;
    let now = DateTime::UNIX_EPOCH + TimeDelta::seconds(100);
    for (id, age) in [("local", 99), ("stale", 31), ("edge", 30), ("future", -100)] {
        registry
            .register(
                node(id),
                now - TimeDelta::seconds(age),
                CancellationToken::new(),
            )
            .await?;
    }
    let mut external = registry.snapshot().await?.ok_or("registry missing")?;
    external
        .nodes
        .get_mut("local")
        .ok_or("local missing")?
        .status = MeshNodeStatus::Updating;
    std::fs::write(
        root.path().join("mesh.json"),
        serde_json::to_vec(&external)?,
    )?;
    let actual = registry
        .heartbeat(
            node("local"),
            now,
            TimeDelta::seconds(30),
            CancellationToken::new(),
        )
        .await?;
    assert!(!actual.nodes.contains_key("stale"));
    assert!(actual.nodes.contains_key("edge"));
    assert!(actual.nodes.contains_key("future"));
    let local = actual.nodes.get("local").ok_or("local missing")?;
    assert_eq!(local.last_seen, now);
    assert_eq!(local.status, MeshNodeStatus::Updating);
    assert_eq!(local.registered_at, now - TimeDelta::seconds(99));
    assert_eq!(actual.updated_at, now + TimeDelta::seconds(100));
    let missing = registry
        .heartbeat(
            node("new"),
            now,
            TimeDelta::seconds(30),
            CancellationToken::new(),
        )
        .await?;
    assert!(missing.nodes.contains_key("new"));
    Ok(())
}

#[tokio::test]
async fn corrupt_and_wrong_typed_files_are_never_replaced() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = registry(&root).await?;
    for text in [
        "{",
        "[]",
        "{}",
        "null",
        r#"{"version":1,"nodes":[],"updatedAt":0}"#,
        r#"{"version":1,"nodes":42,"updatedAt":0}"#,
        r#"{"version":1,"nodes":{},"updatedAt":"today"}"#,
        r#"{"version":2,"nodes":{},"updatedAt":0}"#,
    ] {
        std::fs::write(root.path().join("mesh.json"), text)?;
        assert!(
            registry
                .register(node("local"), Utc::now(), CancellationToken::new())
                .await
                .is_err(),
            "{text}"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("mesh.json"))?,
            text
        );
        assert!(!root.path().join("mesh.json.lock").exists());
    }
    Ok(())
}

#[tokio::test]
async fn cancellation_and_writer_contention_do_not_modify_registry() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = registry(&root).await?;
    let initial = registry
        .register(node("local"), Utc::now(), CancellationToken::new())
        .await?;
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        registry
            .register(node("other"), Utc::now(), cancelled)
            .await,
        Err(RegistryError::Cancelled)
    ));
    std::fs::create_dir(root.path().join("mesh.json.lock"))?;
    assert!(matches!(
        registry
            .unregister("local".to_owned(), Utc::now(), CancellationToken::new())
            .await,
        Err(RegistryError::Busy)
    ));
    assert_eq!(registry.snapshot().await?, Some(initial));
    std::fs::remove_dir(root.path().join("mesh.json.lock"))?;
    let empty = registry
        .unregister("local".to_owned(), Utc::now(), CancellationToken::new())
        .await?;
    assert!(empty.nodes.is_empty());
    Ok(())
}

#[tokio::test]
async fn paths_reject_parent_components_and_shared_prefix_siblings() -> TestResult {
    let root = tempfile::tempdir()?;
    let inside = root.path().join("state");
    let sibling = root.path().join("state-other");
    std::fs::create_dir(&inside)?;
    std::fs::create_dir(&sibling)?;
    for relative in [
        PathBuf::from("../state-other/mesh.json"),
        sibling.join("mesh.json"),
        PathBuf::from("a/../mesh.json"),
    ] {
        assert!(matches!(
            Registry::open(inside.clone(), relative).await,
            Err(RegistryError::OutsideRoot(_))
        ));
    }
    std::fs::create_dir(inside.join("nested"))?;
    let valid = Registry::open(inside, PathBuf::from("./nested/mesh.json")).await?;
    assert_eq!(valid.snapshot().await?, None);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_outside_the_root_are_refused_before_read_or_write() -> TestResult {
    let root = tempfile::tempdir()?;
    let inside = root.path().join("state");
    let sibling = root.path().join("state-other");
    std::fs::create_dir(&inside)?;
    std::fs::create_dir(&sibling)?;
    std::fs::write(sibling.join("mesh.json"), "external data")?;
    let registry = Registry::open(inside.clone(), PathBuf::from("mesh.json")).await?;
    std::os::unix::fs::symlink(sibling.join("mesh.json"), inside.join("mesh.json"))?;
    assert!(matches!(
        registry.snapshot().await,
        Err(RegistryError::OutsideRoot(_))
    ));
    assert!(matches!(
        registry
            .register(node("local"), Utc::now(), CancellationToken::new())
            .await,
        Err(RegistryError::OutsideRoot(_))
    ));
    assert_eq!(
        std::fs::read_to_string(sibling.join("mesh.json"))?,
        "external data"
    );
    std::fs::remove_file(inside.join("mesh.json"))?;
    std::os::unix::fs::symlink(&sibling, inside.join("nested"))?;
    assert!(matches!(
        Registry::open(inside, PathBuf::from("nested/mesh.json")).await,
        Err(RegistryError::OutsideRoot(_))
    ));
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn heartbeat_reports_events_and_stops_without_a_final_write() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = registry(&root).await?;
    let heartbeat = Heartbeat::new(
        registry.clone(),
        node("local"),
        Duration::from_secs(5),
        TimeDelta::seconds(30),
    )?;
    let mut reports = heartbeat.subscribe();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(heartbeat.run(shutdown.clone()));
    tokio::time::timeout(Duration::from_secs(2), reports.changed()).await??;
    assert!(matches!(
        &*reports.borrow_and_update(),
        HeartbeatReport::Updated(_)
    ));
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::time::timeout(Duration::from_secs(2), reports.changed()).await??;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), task).await???;
    let last = registry.snapshot().await?;
    tokio::time::advance(Duration::from_secs(100)).await;
    assert_eq!(registry.snapshot().await?, last);
    Ok(())
}

#[tokio::test(start_paused = true)]
async fn a_restarted_node_republishes_its_record() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = registry(&root).await?;
    let mut stale = node("local");
    stale.port = 5175;
    stale.capabilities = Vec::new();
    registry
        .register(stale, Utc::now(), CancellationToken::new())
        .await?;
    let mut current = node("local");
    current.port = 3001;
    current.capabilities = vec!["den".to_owned()];
    let heartbeat = Heartbeat::new(
        registry.clone(),
        current,
        Duration::from_secs(5),
        TimeDelta::seconds(30),
    )?;
    let mut reports = heartbeat.subscribe();
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(heartbeat.run(shutdown.clone()));
    tokio::time::timeout(Duration::from_secs(2), reports.changed()).await??;
    shutdown.cancel();
    tokio::time::timeout(Duration::from_secs(2), task).await???;
    let file = registry.snapshot().await?.ok_or("mesh file")?;
    let published = file.nodes.get("local").ok_or("local node")?;
    assert_eq!(published.port, 3001);
    assert_eq!(published.capabilities, vec!["den".to_owned()]);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_writers_never_lose_an_acknowledged_registration() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = registry(&root).await?;
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(17));
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..16 {
        let registry = registry.clone();
        let barrier = barrier.clone();
        tasks.spawn(async move {
            let id = format!("node-{index}");
            barrier.wait().await;
            let result = registry
                .register(node(&id), Utc::now(), CancellationToken::new())
                .await;
            (id, result)
        });
    }
    barrier.wait().await;
    let mut acknowledged = std::collections::BTreeSet::new();
    let mut contended = 0;
    while let Some(task) = tokio::time::timeout(Duration::from_secs(5), tasks.join_next()).await? {
        let (id, result) = task?;
        match result {
            Ok(_) => {
                acknowledged.insert(id);
            }
            Err(RegistryError::Busy) => {
                contended += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let actual = registry.snapshot().await?.ok_or("no successful writer")?;
    assert_eq!(acknowledged.len() + contended, 16);
    assert_eq!(
        actual
            .nodes
            .into_keys()
            .collect::<std::collections::BTreeSet<_>>(),
        acknowledged
    );
    Ok(())
}

#[tokio::test]
async fn mismatched_ids_and_invalid_timing_are_typed_errors() -> TestResult {
    let root = tempfile::tempdir()?;
    let registry = registry(&root).await?;
    let mut file = MeshFile::empty(DateTime::UNIX_EPOCH);
    file.nodes.insert("different".to_owned(), node("local"));
    std::fs::write(root.path().join("mesh.json"), serde_json::to_vec(&file)?)?;
    assert!(matches!(
        registry.snapshot().await,
        Err(RegistryError::NodeId(_))
    ));
    assert!(matches!(
        Heartbeat::new(
            registry.clone(),
            node("local"),
            Duration::ZERO,
            TimeDelta::seconds(30)
        ),
        Err(RegistryError::HeartbeatInterval)
    ));
    assert!(matches!(
        registry
            .heartbeat(
                node("local"),
                Utc::now(),
                TimeDelta::zero(),
                CancellationToken::new()
            )
            .await,
        Err(RegistryError::StaleDuration)
    ));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn atomic_replacement_preserves_operator_file_permissions() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::tempdir()?;
    let registry = registry(&root).await?;
    registry
        .register(node("local"), Utc::now(), CancellationToken::new())
        .await?;
    let path = root.path().join("mesh.json");
    assert_eq!(
        std::fs::metadata(&path)?.permissions().mode() & 0o777,
        0o644
    );
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o660))?;
    registry
        .register(node("peer"), Utc::now(), CancellationToken::new())
        .await?;
    assert_eq!(
        std::fs::metadata(&path)?.permissions().mode() & 0o777,
        0o660
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn an_existing_symlink_within_the_root_updates_its_target() -> TestResult {
    let root = tempfile::tempdir()?;
    let target = root.path().join("shared.json");
    std::fs::write(
        &target,
        serde_json::to_vec(&MeshFile::empty(DateTime::UNIX_EPOCH))?,
    )?;
    std::os::unix::fs::symlink(&target, root.path().join("mesh.json"))?;
    let registry = registry(&root).await?;
    let saved = registry
        .register(node("local"), Utc::now(), CancellationToken::new())
        .await?;
    assert_eq!(
        serde_json::from_slice::<MeshFile>(&std::fs::read(target)?)?,
        saved
    );
    assert!(
        std::fs::symlink_metadata(root.path().join("mesh.json"))?
            .file_type()
            .is_symlink()
    );
    Ok(())
}
