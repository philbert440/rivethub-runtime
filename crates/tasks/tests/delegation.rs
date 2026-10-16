#[expect(
    dead_code,
    reason = "each test binary includes the shared harness and uses a different subset"
)]
#[expect(
    clippy::expect_used,
    reason = "a shared harness reports setup failure by panicking; the message names the step"
)]
mod common;
mod delegation_store;

use delegation_store::NotifyStore;

use std::sync::Arc;
use std::time::Duration;

use chrono::DateTime;
use chrono::Utc;
use common::deadline;
use common::settings;
use kernel::tasks::AgentDirectory;
use kernel::tasks::AgentNode;
use kernel::tasks::DelegateRequest;
use kernel::tasks::Delegation;
use kernel::tasks::DelegationError;
use kernel::tasks::PresetAgent;
use kernel::tasks::RuntimeAgent;
use tasks::Directory;
use tasks::Engine;
use tasks::Error;
use tasks::LocalAgent;
use tasks::TargetInput;
use tasks::TaskOrigin;
use testkit::TaskMemory;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use url::Url;
use uuid::Uuid;
use wire::gateway::agents::AgentPreset;
use wire::harness::HarnessId;
use wire::mesh::MeshNodeStatus;
use wire::task::TaskExecutorKind;
use wire::task::TaskStatus;

#[expect(
    clippy::expect_used,
    reason = "the local base url is a fixed literal; a parse failure is a broken fixture"
)]
fn fresh() -> Directory {
    Directory::new("local", Url::parse("http://127.0.0.1:9/").expect("url")).expect("directory")
}

#[expect(
    clippy::expect_used,
    reason = "the preset stamp is a fixed instant; a parse failure is a broken fixture"
)]
fn preset(name: &str, id: &str, harness: Option<HarnessId>, node: Option<&str>) -> AgentPreset {
    let stamp = DateTime::<Utc>::from_timestamp_millis(1_760_000_001_000).expect("stamp");
    AgentPreset {
        id: id.to_owned(),
        name: name.to_owned(),
        color: "blue".to_owned(),
        harness_id: harness,
        model: "demo".to_owned(),
        effort: "low".to_owned(),
        system_prompt: "be brief".to_owned(),
        node: node.map(ToOwned::to_owned),
        directory: None,
        shared_link: None,
        node_base_url: None,
        sort_order: None,
        created_at: stamp,
        updated_at: stamp,
        extra: wire::Extra::default(),
    }
}

#[expect(
    clippy::expect_used,
    reason = "the host stamp is a fixed instant; a parse failure is a broken fixture"
)]
fn host(directory: &mut Directory, id: &str, agents: &[&str], seen: i64, status: MeshNodeStatus) {
    let mut node = common::node(id, "203.0.113.9", 9, agents, status);
    node.last_seen = DateTime::<Utc>::from_timestamp_millis(seen).expect("seen");
    directory.mesh_node(node);
}

fn request(target: &str, timeout: Duration) -> DelegateRequest {
    DelegateRequest {
        requested_by: None,
        target: target.to_owned(),
        prompt: "ship it".to_owned(),
        model: None,
        node: None,
        timeout,
    }
}

#[expect(
    clippy::expect_used,
    reason = "these callers assert the resolved target; a refusal is a broken fixture"
)]
fn chosen(directory: &Directory, target: &str, node: Option<&str>) -> TargetInput {
    directory
        .delegation_target(target, node)
        .expect("target")
        .input
}

#[test]
fn a_harness_preset_wins_over_a_runtime_agent_with_the_same_id() {
    let mut directory = fresh();
    directory.preset(preset("worker", "worker", Some(HarnessId::Codex), None));
    host(
        &mut directory,
        "edge",
        &["worker"],
        1_760_000_002_000,
        MeshNodeStatus::Online,
    );
    let target = chosen(&directory, "worker", None);
    assert_eq!(target.executor, TaskExecutorKind::HarnessSession);
    assert_eq!(target.agent_id, "worker");
    assert_eq!(target.node_affinity, None);
}

#[test]
fn a_preset_id_is_the_preset_name_when_the_preset_wins() {
    let mut directory = fresh();
    directory.preset(preset("named", "preset-id", Some(HarnessId::Codex), None));
    let target = chosen(&directory, "preset-id", None);
    assert_eq!(target.executor, TaskExecutorKind::HarnessSession);
    assert_eq!(target.agent_id, "named");
}

#[test]
fn a_harnessless_preset_yields_the_newest_runtime_host() {
    let mut directory = fresh();
    directory.preset(preset("worker", "preset-id", None, Some("edge")));
    host(
        &mut directory,
        "old",
        &["preset-id"],
        1_760_000_001_000,
        MeshNodeStatus::Online,
    );
    host(
        &mut directory,
        "new",
        &["preset-id"],
        1_760_000_009_000,
        MeshNodeStatus::Online,
    );
    let target = chosen(&directory, "preset-id", None);
    assert_eq!(target.executor, TaskExecutorKind::ChatLoop);
    assert_eq!(target.agent_id, "preset-id");
    assert_eq!(target.node_affinity.as_deref(), Some("new"));
}

#[test]
fn equal_timestamps_pick_the_lowest_node_id() {
    let mut directory = fresh();
    host(
        &mut directory,
        "b-node",
        &["worker"],
        1_760_000_004_000,
        MeshNodeStatus::Online,
    );
    host(
        &mut directory,
        "a-node",
        &["worker"],
        1_760_000_004_000,
        MeshNodeStatus::Online,
    );
    directory.local_agent("other", common::plain_agent());
    let target = chosen(&directory, "worker", None);
    assert_eq!(target.node_affinity.as_deref(), Some("a-node"));
}

#[test]
fn an_exact_preset_name_containing_at_is_not_split() {
    let mut directory = fresh();
    directory.preset(preset("a@b", "a@b", Some(HarnessId::Codex), None));
    host(
        &mut directory,
        "b",
        &["a"],
        1_760_000_009_000,
        MeshNodeStatus::Online,
    );
    let target = chosen(&directory, "a@b", None);
    assert_eq!(target.executor, TaskExecutorKind::HarnessSession);
    assert_eq!(target.agent_id, "a@b");
    assert_eq!(target.node_affinity, None);
}

#[test]
fn a_pin_selects_the_named_host_even_when_it_is_older() {
    let mut directory = fresh();
    host(
        &mut directory,
        "old",
        &["worker"],
        1_760_000_001_000,
        MeshNodeStatus::Online,
    );
    host(
        &mut directory,
        "new",
        &["worker"],
        1_760_000_009_000,
        MeshNodeStatus::Online,
    );
    let target = chosen(&directory, "worker@old", None);
    assert_eq!(target.executor, TaskExecutorKind::ChatLoop);
    assert_eq!(target.node_affinity.as_deref(), Some("old"));
}

#[test]
fn a_pin_that_is_not_a_host_uses_the_preset_then_prepare() {
    let mut directory = fresh();
    directory.preset(preset("worker", "worker", Some(HarnessId::Codex), None));
    host(
        &mut directory,
        "edge",
        &["other"],
        1_760_000_001_000,
        MeshNodeStatus::Offline,
    );
    let target = chosen(&directory, "worker@edge", None);
    assert_eq!(target.executor, TaskExecutorKind::HarnessSession);
    assert_eq!(target.node_affinity.as_deref(), Some("edge"));
    assert!(matches!(
        directory.prepare(&target).expect_err("offline"),
        Error::MeshUnavailable { .. }
    ));
}

#[test]
fn unknown_ambiguous_and_conflicting_targets_are_refused() {
    let directory = fresh();
    assert!(matches!(
        directory
            .delegation_target("missing", None)
            .expect_err("unknown"),
        Error::UnknownTarget { .. }
    ));
    assert!(matches!(
        directory.delegation_target("", None).expect_err("empty"),
        Error::AgentRequired
    ));
    assert!(matches!(
        directory
            .delegation_target("worker", Some(""))
            .expect_err("empty node"),
        Error::MeshAddress
    ));
    assert!(matches!(
        directory
            .delegation_target("agent@", None)
            .expect_err("empty node side"),
        Error::AgentRequired
    ));
    assert!(matches!(
        directory
            .delegation_target("@node", None)
            .expect_err("empty agent side"),
        Error::AgentRequired
    ));
    assert!(matches!(
        directory
            .delegation_target("worker@edge", Some("other"))
            .expect_err("conflict"),
        Error::ConflictingNode { .. }
    ));
    let mut directory = fresh();
    directory.local_agent("worker", common::plain_agent());
    directory.preset(preset("worker", "worker", None, None));
    let target = chosen(&directory, "worker", None);
    assert!(matches!(
        directory.prepare(&target).expect_err("ambiguous"),
        Error::AmbiguousTarget { .. }
    ));
}

#[test]
fn a_pinned_node_outside_the_roster_is_mesh_agent_when_other_hosts_exist() {
    let mut directory = fresh();
    host(
        &mut directory,
        "edge",
        &["worker"],
        1_760_000_001_000,
        MeshNodeStatus::Online,
    );
    assert!(matches!(
        directory
            .delegation_target("worker@missing", None)
            .expect_err("mesh"),
        Error::MeshAgent { .. }
    ));
}

#[test]
fn the_catalog_lists_presets_and_online_hosts_in_order() {
    let mut directory = fresh();
    directory.local_agent("zeta", plain());
    directory.local_agent("alpha", plain());
    directory.preset(preset("bravo", "bravo-id", None, None));
    directory.preset(preset(
        "alpha-preset",
        "alpha-id",
        Some(HarnessId::Codex),
        Some("down"),
    ));
    host(
        &mut directory,
        "local",
        &["ghost", "alpha"],
        1_760_000_003_000,
        MeshNodeStatus::Online,
    );
    host(
        &mut directory,
        "old",
        &["mid"],
        1_760_000_001_000,
        MeshNodeStatus::Online,
    );
    host(
        &mut directory,
        "new",
        &["mid"],
        1_760_000_009_000,
        MeshNodeStatus::Online,
    );
    host(
        &mut directory,
        "down",
        &["gone"],
        1_760_000_009_000,
        MeshNodeStatus::Offline,
    );
    let seen = DateTime::<Utc>::from_timestamp_millis(1_760_000_003_000).expect("seen");
    let older = DateTime::<Utc>::from_timestamp_millis(1_760_000_001_000).expect("older");
    let newer = DateTime::<Utc>::from_timestamp_millis(1_760_000_009_000).expect("newer");
    assert_eq!(directory.catalog(), listed(seen, older, newer));
}

fn listed(seen: DateTime<Utc>, older: DateTime<Utc>, newer: DateTime<Utc>) -> AgentDirectory {
    AgentDirectory {
        presets: vec![
            PresetAgent {
                id: "alpha-id".to_owned(),
                name: "alpha-preset".to_owned(),
                node: Some("down".to_owned()),
                harness: Some(HarnessId::Codex),
                directory: None,
                local: false,
            },
            PresetAgent {
                id: "bravo-id".to_owned(),
                name: "bravo".to_owned(),
                node: None,
                harness: None,
                directory: None,
                local: true,
            },
        ],
        runtime: vec![
            RuntimeAgent {
                id: "alpha".to_owned(),
                nodes: vec![AgentNode {
                    id: "local".to_owned(),
                    last_seen: Some(seen),
                }],
            },
            RuntimeAgent {
                id: "mid".to_owned(),
                nodes: vec![
                    AgentNode {
                        id: "new".to_owned(),
                        last_seen: Some(newer),
                    },
                    AgentNode {
                        id: "old".to_owned(),
                        last_seen: Some(older),
                    },
                ],
            },
            RuntimeAgent {
                id: "zeta".to_owned(),
                nodes: vec![AgentNode {
                    id: "local".to_owned(),
                    last_seen: Some(seen),
                }],
            },
        ],
    }
}

fn plain() -> LocalAgent {
    common::plain_agent()
}

#[expect(
    clippy::expect_used,
    reason = "engine construction failure is a broken fixture"
)]
fn watching(
    directory: Directory,
) -> (
    Engine<NotifyStore, TaskMemory>,
    watch::Receiver<Option<Uuid>>,
) {
    let (sender, receiver) = watch::channel(None);
    let engine = Engine::new(
        NotifyStore {
            inner: TaskMemory::new(),
            inserted: sender,
        },
        TaskMemory::new(),
        directory,
        settings(None, std::collections::BTreeMap::new()),
    )
    .expect("engine");
    (engine, receiver)
}

#[tokio::test]
async fn a_cancelled_call_kills_the_row() {
    let (engine, mut inserted) = watching(common::directory(
        Url::parse("http://127.0.0.1:9/").expect("url"),
    ));
    let engine = Arc::new(engine);
    let cancel = CancellationToken::new();
    let running = {
        let engine = Arc::clone(&engine);
        let cancel = cancel.clone();
        tokio::spawn(async move {
            engine
                .delegate(request("worker", Duration::from_secs(30)), cancel)
                .await
        })
    };
    let id = tokio::time::timeout(deadline(), inserted.wait_for(Option::is_some))
        .await
        .expect("inserted")
        .expect("watch")
        .expect("id");
    cancel.cancel();
    let error = tokio::time::timeout(deadline(), running)
        .await
        .expect("joined")
        .expect("spawn")
        .expect_err("cancelled");
    assert!(matches!(error, DelegationError::Cancelled));
    let loaded = engine.load(id).await.expect("load");
    assert_eq!(loaded.status, TaskStatus::Killed);
    assert_eq!(loaded.error.as_deref(), Some("Killed by parent"));
}

#[tokio::test]
async fn cancelling_before_submit_writes_no_row() {
    let (engine, inserted) = watching(common::directory(
        Url::parse("http://127.0.0.1:9/").expect("url"),
    ));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = engine
        .delegate(request("worker", Duration::from_secs(30)), cancel)
        .await
        .expect_err("cancelled");
    assert!(matches!(error, DelegationError::Cancelled));
    assert!(inserted.borrow().is_none());
}

#[tokio::test]
async fn a_zero_timeout_is_refused_before_submit() {
    let (engine, inserted) = watching(common::directory(
        Url::parse("http://127.0.0.1:9/").expect("url"),
    ));
    let error = engine
        .delegate(request("worker", Duration::ZERO), CancellationToken::new())
        .await
        .expect_err("range");
    assert!(
        matches!(error, DelegationError::Rejected(message) if message == "timeout is out of range")
    );
    assert!(inserted.borrow().is_none());
}

#[tokio::test]
async fn a_deadline_returns_the_queued_row_without_killing_it() {
    let engine = common::engine(Url::parse("http://127.0.0.1:9/").expect("url"));
    let outcome = tokio::time::timeout(
        deadline(),
        engine.delegate(
            request("worker", Duration::from_millis(40)),
            CancellationToken::new(),
        ),
    )
    .await
    .expect("deadline")
    .expect("delegate");
    assert!(outcome.timed_out);
    assert_eq!(outcome.status, TaskStatus::Queued);
    let loaded = engine.load(outcome.id).await.expect("load");
    assert_eq!(loaded.status, TaskStatus::Queued);
    assert_eq!(loaded.origin, TaskOrigin::Tool);
    assert_eq!(loaded.budget.max_wall_clock_ms, Some(40));
}

#[tokio::test]
async fn a_harness_preset_is_what_gets_submitted() {
    let mut directory = fresh();
    directory.preset(preset("worker", "worker", Some(HarnessId::Codex), None));
    let engine = Engine::new(
        TaskMemory::new(),
        TaskMemory::new(),
        directory,
        settings(None, std::collections::BTreeMap::new()),
    )
    .expect("engine");
    let outcome = tokio::time::timeout(
        deadline(),
        engine.delegate(
            request("worker", Duration::from_millis(40)),
            CancellationToken::new(),
        ),
    )
    .await
    .expect("deadline")
    .expect("delegate");
    let loaded = engine.load(outcome.id).await.expect("load");
    assert_eq!(loaded.executor, TaskExecutorKind::HarnessSession);
    assert_eq!(loaded.agent_id, "worker");
    assert_eq!(loaded.origin, TaskOrigin::Tool);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_served_delegation_returns_the_model_and_the_reply() {
    let (stub, _rx) = common::chat_stub(common::Script::reply("done")).await;
    let engine = Arc::new(common::engine(common::base_url(stub.addr)));
    let cancel = CancellationToken::new();
    let worker = {
        let engine = Arc::clone(&engine);
        let cancel = cancel.clone();
        tokio::spawn(async move { engine.serve(cancel).await })
    };
    let mut created = request("worker", Duration::from_secs(30));
    created.model = Some("demo-model".to_owned());
    let outcome = tokio::time::timeout(
        Duration::from_secs(15),
        engine.delegate(created, CancellationToken::new()),
    )
    .await
    .expect("deadline")
    .expect("delegate");
    assert!(!outcome.timed_out);
    assert_eq!(outcome.status, TaskStatus::Completed);
    assert_eq!(
        outcome
            .result
            .as_ref()
            .map(|result| result.summary.as_str()),
        Some("done")
    );
    let loaded = engine.load(outcome.id).await.expect("load");
    assert_eq!(loaded.spec.model.as_deref(), Some("demo-model"));
    assert_eq!(loaded.executor, TaskExecutorKind::ChatLoop);
    assert_eq!(loaded.budget.max_wall_clock_ms, Some(30_000));
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(15), worker)
        .await
        .expect("serve joined")
        .expect("spawn")
        .expect("serve");
    stub.close().await.expect("stub closed");
}
