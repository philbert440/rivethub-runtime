use std::collections::BTreeMap;

use chrono::DateTime;
use wire::gateway::agents::AgentPreset;
use wire::harness::HarnessId;
use wire::mesh::MeshNode;
use wire::mesh::MeshNodeStatus;

use super::LocalAgent;
use super::Refusal;
use super::Sources;

fn preset(name: &str, node: Option<&str>, harness: Option<HarnessId>) -> AgentPreset {
    AgentPreset {
        id: format!("id-{name}"),
        name: name.to_owned(),
        color: String::new(),
        harness_id: harness,
        model: "default".to_owned(),
        effort: String::new(),
        system_prompt: String::new(),
        node: node.map(str::to_owned),
        directory: None,
        shared_link: None,
        node_base_url: None,
        sort_order: None,
        created_at: DateTime::UNIX_EPOCH,
        updated_at: DateTime::UNIX_EPOCH,
        extra: wire::Extra::default(),
    }
}

fn mesh(id: &str, agents: &[&str], seen: i64, status: MeshNodeStatus) -> MeshNode {
    MeshNode {
        id: id.to_owned(),
        name: id.to_owned(),
        role: None,
        agents: agents.iter().map(|agent| (*agent).to_owned()).collect(),
        host: id.to_owned(),
        port: 5174,
        providers: Vec::new(),
        models: Vec::new(),
        capabilities: Vec::new(),
        status,
        last_seen: DateTime::UNIX_EPOCH + chrono::TimeDelta::seconds(seen),
        registered_at: DateTime::UNIX_EPOCH,
        version: "test".to_owned(),
        metadata: None,
        ssh_user: None,
        install_root: None,
        platform: None,
        extra: wire::Extra::default(),
    }
}

fn sources(local: &BTreeMap<String, LocalAgent>, nodes: Option<Vec<MeshNode>>) -> Sources<'_> {
    Sources {
        node: "a",
        presets: vec![
            preset("coder", Some("b"), Some(HarnessId::ClaudeCode)),
            preset("grok", None, None),
        ],
        local,
        harnessed: BTreeMap::from([("claude".to_owned(), HarnessId::ClaudeCode)]),
        nodes,
    }
}

#[test]
fn a_preset_runs_on_its_node_with_its_harness_and_the_harness_default_model() {
    let local = BTreeMap::new();
    let resolved = sources(&local, None)
        .resolve("coder", None)
        .expect("preset");
    assert_eq!(
        (resolved.node.as_str(), resolved.harness, resolved.model),
        ("b", Some(HarnessId::ClaudeCode), None)
    );
    assert_eq!(resolved.preset.as_deref(), Some("id-coder"));
}

#[test]
fn a_harnessless_preset_yields_to_a_runtime_agent_of_the_same_name() {
    let local = BTreeMap::new();
    let nodes = vec![mesh("c", &["grok"], 10, MeshNodeStatus::Online)];
    let resolved = sources(&local, Some(nodes))
        .resolve("grok", None)
        .expect("runtime");
    assert_eq!((resolved.node.as_str(), resolved.harness), ("c", None));
    assert_eq!(resolved.preset, None);
    let lonely = sources(&local, None).resolve("grok", None);
    assert_eq!(lonely, Err(Refusal::PresetHarness("grok".to_owned())));
}

#[test]
fn a_runtime_agent_runs_on_its_newest_host_unless_pinned() {
    let local = BTreeMap::from([("claude".to_owned(), LocalAgent::default())]);
    let nodes = vec![
        mesh("a", &["claude"], 5, MeshNodeStatus::Online),
        mesh("c", &["claude"], 10, MeshNodeStatus::Online),
        mesh("d", &["claude"], 20, MeshNodeStatus::Offline),
    ];
    let view = sources(&local, Some(nodes));
    assert_eq!(view.resolve("claude", None).expect("newest").node, "c");
    let pinned = view.resolve("claude@a", None).expect("pinned");
    assert_eq!(
        (pinned.node.as_str(), pinned.harness),
        ("a", Some(HarnessId::ClaudeCode))
    );
    assert_eq!(
        view.resolve("claude@a", Some("c")),
        Err(Refusal::ConflictingNode {
            target: "claude@a".to_owned(),
            embedded: "a".to_owned(),
            explicit: "c".to_owned(),
        })
    );
    assert_eq!(
        view.resolve("coder", None),
        Err(Refusal::Offline("b".to_owned()))
    );
}

#[test]
fn a_local_agent_without_a_harness_is_not_delegable() {
    let local = BTreeMap::from([("local".to_owned(), LocalAgent::default())]);
    assert_eq!(
        sources(&local, None).resolve("local", None),
        Err(Refusal::NotHarnessed("local".to_owned()))
    );
    assert_eq!(
        sources(&local, None).resolve("nobody", None),
        Err(Refusal::Unknown("nobody".to_owned()))
    );
}
