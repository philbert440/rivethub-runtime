use std::sync::Arc;

use chrono::DateTime;
use chrono::Utc;
use kernel::harness::Command;
use kernel::harness::HarnessCommands;
use kernel::tasks::CatalogList;
use kernel::tasks::CatalogSource;
use kernel::tasks::TaskError;
use tasks::Catalog;
use tasks::Directory;
use tasks::LocalAgent;
use url::Url;
use wire::gateway::agents::AgentPreset;
use wire::gateway::catalog::CatalogAgent;
use wire::gateway::catalog::CatalogCommand;
use wire::gateway::catalog::CatalogExecutorEntry;
use wire::harness::HarnessId;
use wire::task::HarnessExecutorCapabilities;
use wire::task::McpInjection;

#[tokio::test]
async fn agents_come_from_the_directory() {
    let agents = catalog(false, Vec::new()).agents().await.expect("agents");
    let mut agents = agents.into_iter();
    assert!(matches!(
        agents.next(),
        Some(CatalogAgent::Preset(preset))
            if preset.name == "alpha"
                && preset.local
                && preset.node == "local"
                && preset.implemented == Some(true)
                && preset.harness_id == Some(HarnessId::ClaudeCode)
                && preset.model.as_deref() == Some("demo")
                && preset.effort.as_deref() == Some("low")
    ));
    assert!(matches!(
        agents.next(),
        Some(CatalogAgent::Preset(preset))
            if preset.name == "remote" && !preset.local && preset.node == "edge"
    ));
    assert!(matches!(
        agents.next(),
        Some(CatalogAgent::Node(node))
            if node.id == "main"
                && node.local
                && node.node == "local"
                && node.provider.as_deref() == Some("p")
                && node.model.as_deref() == Some("m")
    ));
    assert!(agents.next().is_none());
}

#[tokio::test]
async fn each_offered_executor_declares_the_session_runner() {
    let local = catalog(false, Vec::new())
        .executors()
        .await
        .expect("executors");
    assert_eq!(keys(&local), ["chat-loop", "harness-session"]);
    let chat = runner_of(&local, "chat-loop");
    let harness = runner_of(&local, "harness-session");
    assert_eq!(chat, harness);
    pin_session_runner(&chat);
    let meshed = catalog(true, Vec::new())
        .executors()
        .await
        .expect("mesh executors");
    assert_eq!(keys(&meshed), ["chat-loop", "harness-session", "mesh"]);
    assert_eq!(runner_of(&meshed, "mesh"), chat);
}

#[tokio::test]
async fn tools_are_the_registry_and_harness_commands_come_from_discovery() {
    let named = catalog(false, vec!["zeta".to_owned(), "alpha".to_owned()]);
    assert_eq!(
        named.tools().await.expect("tools"),
        vec!["zeta".to_owned(), "alpha".to_owned()]
    );
    assert_eq!(named.skills().await.expect("skills"), Vec::new());
    for key in ["chat-loop", "harness-session", "mesh"] {
        assert_eq!(named.commands(key).await.expect(key), Vec::new());
    }
    let missing = named
        .commands("missing")
        .await
        .expect_err("unknown executor");
    assert_eq!(missing.to_string(), "harness commands are unavailable");
    assert!(matches!(
        missing,
        TaskError::UnavailableCatalog(CatalogList::Commands)
    ));
    let harness = Arc::new(testkit::FakeHarness::new(Vec::new()));
    harness.set_commands(vec![HarnessCommands {
        harness: HarnessId::ClaudeCode,
        commands: vec![Command {
            name: "review".to_owned(),
            description: "review the change".to_owned(),
        }],
    }]);
    let discovered = Catalog::new(&sample(), false, Vec::new(), Some(harness));
    assert_eq!(
        discovered
            .commands("harness-session")
            .await
            .expect("harness"),
        vec![CatalogCommand {
            name: "review".to_owned(),
            description: "review the change".to_owned(),
            arg_hint: None,
            source: "claude-code".to_owned(),
        }]
    );
    assert_eq!(
        discovered.commands("chat-loop").await.expect("chat"),
        Vec::new()
    );
    assert_eq!(
        catalog(false, Vec::new())
            .tools()
            .await
            .expect("empty registry"),
        Vec::<String>::new()
    );
}

fn catalog(mesh: bool, tools: Vec<String>) -> Catalog {
    Catalog::new(&sample(), mesh, tools, None)
}

fn keys(entries: &[CatalogExecutorEntry]) -> Vec<&str> {
    entries.iter().map(|entry| entry.key.as_str()).collect()
}

#[expect(
    clippy::expect_used,
    reason = "a missing executor key means the catalog stopped offering that runner"
)]
fn runner_of(entries: &[CatalogExecutorEntry], key: &str) -> HarnessExecutorCapabilities {
    let entry = entries.iter().find(|entry| entry.key == key).expect(key);
    assert_eq!(entry.implemented, Some(true));
    assert_eq!(entry.harness_id, None);
    assert_eq!(entry.commands, Vec::new());
    entry.capabilities.clone()
}

fn pin_session_runner(capabilities: &HarnessExecutorCapabilities) {
    assert!(capabilities.turn.steerable);
    assert!(capabilities.turn.multi_turn);
    assert!(!capabilities.turn.structured_stream);
    assert!(capabilities.result.usage_in_result);
    assert!(!capabilities.result.session_id_capture);
    assert!(!capabilities.interaction.slash_commands);
    assert!(!capabilities.interaction.effort_selection);
    assert_eq!(capabilities.mcp_injection, McpInjection::None);
}

#[expect(
    clippy::expect_used,
    reason = "the local base url is a fixed literal; a parse failure is a broken fixture"
)]
fn sample() -> Directory {
    let mut directory = Directory::new("local", Url::parse("http://127.0.0.1:9/").expect("url"))
        .expect("directory");
    directory.preset(preset("remote", Some("edge"), None));
    directory.preset(preset("alpha", None, Some(HarnessId::ClaudeCode)));
    directory.local_agent(
        "main",
        LocalAgent {
            model: Some("m".to_owned()),
            provider: Some("p".to_owned()),
            effort: None,
            harness: None,
            system_prompt: None,
            working_dir: None,
        },
    );
    directory
}

#[expect(
    clippy::expect_used,
    reason = "the preset stamp is a fixed instant; a parse failure is a broken fixture"
)]
fn preset(name: &str, node: Option<&str>, harness: Option<HarnessId>) -> AgentPreset {
    let stamp = DateTime::<Utc>::from_timestamp_millis(1_760_000_001_000).expect("stamp");
    AgentPreset {
        id: format!("{name}-id"),
        name: name.to_owned(),
        color: "blue".to_owned(),
        harness_id: harness,
        model: "demo".to_owned(),
        effort: "low".to_owned(),
        system_prompt: "be brief".to_owned(),
        node: node.map(str::to_owned),
        directory: None,
        shared_link: None,
        node_base_url: None,
        sort_order: None,
        created_at: stamp,
        updated_at: stamp,
        extra: wire::Extra::default(),
    }
}
