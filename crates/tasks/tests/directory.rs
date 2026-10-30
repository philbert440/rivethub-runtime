#[expect(
    dead_code,
    reason = "each test binary includes the shared harness and uses a different subset"
)]
#[expect(
    clippy::expect_used,
    reason = "a shared harness reports setup failure by panicking; the message names the step"
)]
mod common;

#[expect(
    dead_code,
    reason = "each test binary includes the certificate mint and uses a different subset"
)]
#[expect(
    clippy::expect_used,
    reason = "certificate minting failure is a broken fixture"
)]
mod mesh_support;

use chrono::DateTime;
use chrono::Utc;
use common::node;
use kernel::queue::JobName;
use tasks::Directory;
use tasks::Error;
use tasks::LocalAgent;
use tasks::Prepared;
use tasks::TargetInput;
use url::Url;
use wire::gateway::agents::AgentPreset;
use wire::harness::HarnessId;
use wire::mesh::MeshNodeStatus;
use wire::task::Effort;
use wire::task::TaskExecutorKind;

fn input(executor: TaskExecutorKind, agent: &str) -> TargetInput {
    TargetInput {
        executor,
        executor_target: None,
        node_affinity: None,
        agent_id: agent.to_string(),
    }
}

#[expect(
    clippy::expect_used,
    reason = "the preset stamp is a fixed instant; a parse failure is a broken fixture"
)]
fn preset(name: &str, node_name: Option<&str>, effort: &str, base: Option<&str>) -> AgentPreset {
    let stamp = DateTime::<Utc>::from_timestamp_millis(1_760_000_001_000).expect("stamp");
    AgentPreset {
        id: name.to_string(),
        name: name.to_string(),
        color: "blue".to_string(),
        harness_id: None,
        model: "demo".to_string(),
        effort: effort.to_string(),
        system_prompt: "be brief".to_string(),
        node: node_name.map(ToString::to_string),
        directory: None,
        shared_link: None,
        node_base_url: base.map(ToString::to_string),
        sort_order: None,
        created_at: stamp,
        updated_at: stamp,
        extra: wire::Extra::default(),
    }
}

#[expect(
    clippy::expect_used,
    reason = "the local base url is a fixed literal; a parse failure is a broken fixture"
)]
fn fresh() -> Directory {
    Directory::new("local", Url::parse("http://127.0.0.1:9/").expect("url")).expect("directory")
}

fn mesh(directory: &mut Directory, id: &str, host: &str, status: MeshNodeStatus) {
    directory.mesh_node(node(id, host, 9, &["worker"], status));
    directory.set_mesh_identity(Some(mesh_support::sample()));
}

#[test]
fn a_local_agent_stays_on_this_node() {
    let mut directory = fresh();
    directory.local_agent("worker", common::plain_agent());
    let prepared = directory
        .prepare(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect("prepare");
    assert_eq!(
        prepared,
        Prepared {
            node: "local".to_string(),
            remote: false,
            job_name: JobName::RunTaskOn {
                node: "local".to_string(),
            },
        }
    );
    let resolved = directory
        .resolve(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect("resolve");
    assert!(resolved.mesh.is_none());
    assert!(!resolved.remote);
}

#[test]
fn a_preset_on_another_node_is_a_remote_job() {
    let mut directory = fresh();
    directory.preset(preset("worker", Some("edge"), "low", None));
    mesh(
        &mut directory,
        "edge",
        "203.0.113.9",
        MeshNodeStatus::Online,
    );
    let prepared = directory
        .prepare(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect("prepare");
    assert_eq!(
        prepared.job_name,
        JobName::RunTaskOn {
            node: "edge".to_string(),
        }
    );
    let resolved = directory
        .resolve(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect("resolve");
    assert_eq!(resolved.mesh, Some(mesh_support::sample()));
    assert_eq!(resolved.base.as_str(), "https://203.0.113.9:9/");
    assert_eq!(resolved.effort, Some(Effort::Low));
    assert_eq!(resolved.system_prompt.as_deref(), Some("be brief"));
}

#[test]
fn the_same_name_cannot_be_both_a_local_agent_and_a_preset() {
    let mut directory = fresh();
    directory.local_agent("worker", common::plain_agent());
    directory.preset(preset("worker", None, "medium", None));
    let error = directory
        .prepare(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect_err("ambiguous");
    assert!(matches!(error, Error::AmbiguousTarget { .. }));
}

#[test]
fn an_unknown_agent_is_refused() {
    let directory = fresh();
    let error = directory
        .prepare(&input(TaskExecutorKind::ChatLoop, "missing"))
        .expect_err("unknown");
    assert!(matches!(error, Error::UnknownTarget { .. }));
}

#[test]
fn an_offline_node_or_an_unlisted_runtime_agent_is_refused() {
    let mut directory = fresh();
    directory.preset(preset("worker", Some("edge"), "high", None));
    mesh(
        &mut directory,
        "edge",
        "203.0.113.9",
        MeshNodeStatus::Offline,
    );
    assert!(matches!(
        directory
            .prepare(&input(TaskExecutorKind::ChatLoop, "worker"))
            .expect_err("offline"),
        Error::MeshUnavailable { .. }
    ));
    let mut directory = fresh();
    directory.preset(preset("worker", Some("edge"), "high", None));
    directory.mesh_node(node(
        "edge",
        "203.0.113.9",
        9,
        &["other"],
        MeshNodeStatus::Degraded,
    ));
    assert!(matches!(
        directory
            .prepare(&input(TaskExecutorKind::ChatLoop, "worker"))
            .expect_err("degraded"),
        Error::MeshUnavailable { .. }
    ));
    let mut directory = fresh();
    directory.mesh_node(node(
        "edge",
        "203.0.113.9",
        9,
        &["other"],
        MeshNodeStatus::Online,
    ));
    directory.set_mesh_identity(Some(mesh_support::sample()));
    let mut pinned = input(TaskExecutorKind::ChatLoop, "worker");
    pinned.node_affinity = Some("edge".to_string());
    assert!(matches!(
        directory.prepare(&pinned).expect_err("roster"),
        Error::MeshAgent { .. }
    ));
    directory.preset(preset("worker", Some("edge"), "high", None));
    let prepared = directory
        .prepare(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect("a preset needs only its node online");
    assert_eq!(
        prepared.job_name,
        JobName::RunTaskOn {
            node: "edge".to_string(),
        }
    );
}

#[test]
fn an_explicit_node_wins_over_the_preset_node() {
    let mut directory = fresh();
    directory.preset(preset("worker", Some("edge"), "low", None));
    mesh(
        &mut directory,
        "edge",
        "203.0.113.9",
        MeshNodeStatus::Online,
    );
    mesh(
        &mut directory,
        "other",
        "203.0.113.10",
        MeshNodeStatus::Online,
    );
    let mut pinned = input(TaskExecutorKind::ChatLoop, "worker");
    pinned.node_affinity = Some("other".to_string());
    let prepared = directory.prepare(&pinned).expect("pinned");
    assert_eq!(prepared.node, "other");
    assert!(prepared.remote);
    let prepared = directory
        .prepare(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect("preset node");
    assert_eq!(prepared.node, "edge");
    let mut empty = input(TaskExecutorKind::ChatLoop, "worker");
    empty.node_affinity = Some(String::new());
    assert!(matches!(
        directory.prepare(&empty).expect_err("empty node"),
        Error::MeshAddress
    ));
}

#[test]
fn https_without_a_mesh_identity_is_refused() {
    let mut directory = fresh();
    directory.preset(preset("worker", Some("edge"), "low", None));
    directory.mesh_node(node(
        "edge",
        "203.0.113.9",
        9,
        &["worker"],
        MeshNodeStatus::Online,
    ));
    assert!(matches!(
        directory
            .prepare(&input(TaskExecutorKind::ChatLoop, "worker"))
            .expect_err("identity"),
        Error::MeshTlsRequired
    ));
}

#[test]
fn mesh_addresses_reject_userinfo_bad_hosts_and_accept_ipv6() {
    let mut directory = fresh();
    directory.preset(preset(
        "worker",
        Some("edge"),
        "low",
        Some("http://user:pass@203.0.113.9/"),
    ));
    mesh(
        &mut directory,
        "edge",
        "203.0.113.9",
        MeshNodeStatus::Online,
    );
    assert!(matches!(
        directory
            .resolve(&input(TaskExecutorKind::ChatLoop, "worker"))
            .expect_err("userinfo"),
        Error::BaseUrl
    ));
    let mut directory = fresh();
    directory.preset(preset("worker", Some("edge"), "low", None));
    mesh(&mut directory, "edge", "bad host", MeshNodeStatus::Online);
    assert!(matches!(
        directory
            .resolve(&input(TaskExecutorKind::ChatLoop, "worker"))
            .expect_err("host"),
        Error::MeshAddress
    ));
    let mut directory = fresh();
    directory.preset(preset("worker", Some("edge"), "low", None));
    mesh(&mut directory, "edge", "::1", MeshNodeStatus::Online);
    let resolved = directory
        .resolve(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect("ipv6");
    assert_eq!(resolved.base.as_str(), "https://[::1]:9/");
}

#[test]
fn a_mesh_executor_needs_a_node_and_a_local_node_stays_a_local_job() {
    let mut directory = fresh();
    let error = directory
        .prepare(&input(TaskExecutorKind::Mesh, "worker"))
        .expect_err("node");
    assert!(matches!(error, Error::MeshAddress));
    directory.local_agent(
        "worker",
        LocalAgent {
            harness: Some(HarnessId::ClaudeCode),
            ..common::plain_agent()
        },
    );
    mesh(&mut directory, "local", "127.0.0.1", MeshNodeStatus::Online);
    let mut mesh_input = input(TaskExecutorKind::Mesh, "worker");
    mesh_input.executor_target = Some("local".to_string());
    let prepared = directory.prepare(&mesh_input).expect("prepare");
    assert_eq!(
        prepared.job_name,
        JobName::RunTaskOn {
            node: "local".to_string(),
        }
    );
    let resolved = directory.resolve(&mesh_input).expect("resolve");
    assert_eq!(resolved.mesh, Some(mesh_support::sample()));
    assert_eq!(resolved.base.as_str(), "https://127.0.0.1:9/");
}

#[test]
fn an_http_preset_base_stays_anonymous() {
    let mut directory = fresh();
    directory.preset(preset(
        "worker",
        Some("edge"),
        "low",
        Some("http://203.0.113.9:9/"),
    ));
    directory.mesh_node(node(
        "edge",
        "203.0.113.9",
        9,
        &["worker"],
        MeshNodeStatus::Online,
    ));
    let resolved = directory
        .resolve(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect("http base");
    assert_eq!(resolved.mesh, None);
    assert_eq!(resolved.base.as_str(), "http://203.0.113.9:9/");
    directory.set_mesh_identity(Some(mesh_support::sample()));
    let resolved = directory
        .resolve(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect("http base with identity");
    assert_eq!(resolved.mesh, None);
}

#[test]
fn an_invalid_preset_effort_is_refused() {
    let mut directory = fresh();
    directory.preset(preset("worker", None, "huge", None));
    let error = directory
        .resolve(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect_err("effort");
    assert!(matches!(error, Error::Stored(_)));
    let mut directory = fresh();
    directory.preset(preset("worker", None, "", Some("/srv/agents/worker")));
    let resolved = directory
        .resolve(&input(TaskExecutorKind::ChatLoop, "worker"))
        .expect("an empty effort is unset");
    assert_eq!(resolved.effort, None);
}
