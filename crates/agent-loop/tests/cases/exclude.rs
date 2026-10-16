use std::sync::Arc;

use kernel::agent::excluded_names;
use kernel::model::ModelChunk;
use serde::Deserialize;
use wire::JsonObject;
use wire::task_overrides::TaskOverrides;

use crate::support::provider::{Provider, chunks};
use crate::support::{Harness, agent, call, names, text, turn};

#[derive(Debug, Deserialize)]
struct Fixture {
    rows: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
struct Scenario {
    name: String,
    row: Row,
}

#[derive(Debug, Deserialize)]
struct Row {
    spec: JsonObject,
}

fn production_exclusions() -> Vec<String> {
    let path = testkit::recording("tasks/ts-rows.json");
    let text = std::fs::read_to_string(path).expect("task rows");
    let fixture: Fixture = serde_json::from_str(&text).expect("task rows json");
    let scenario = fixture
        .rows
        .into_iter()
        .find(|scenario| scenario.name == "production-exclude-tools")
        .expect("production row");
    let encoded = serde_json::to_string(&scenario.row.spec).expect("spec");
    let overrides: TaskOverrides = serde_json::from_str(&encoded).expect("overrides");
    overrides.exclude_tools.expect("excludeTools")
}

#[tokio::test]
async fn excluded_tools_are_withheld_and_refused() {
    let excluded = production_exclusions();
    assert!(!excluded.iter().any(|name| name == "echo"));
    let called = excluded.first().expect("excluded tool").clone();
    let mut harness = Harness::new().await;
    let provider = Provider::new(vec![
        chunks(
            [
                call(0, &called, r#"{"text":"secret"}"#),
                vec![ModelChunk::Done],
            ]
            .concat(),
        ),
        chunks(vec![text("kept"), ModelChunk::Done]),
    ]);
    let mut tools: Vec<Arc<dyn kernel::tool::Tool>> =
        vec![Arc::new(testkit::FakeTool::new("echo").expect("echo tool"))];
    for name in &excluded {
        tools.push(Arc::new(
            testkit::FakeTool::new(name).expect("excluded tool"),
        ));
    }
    let runner = agent(provider.clone(), tools, &harness);
    let mut input = turn();
    let mut authorized = excluded.clone();
    authorized.push("echo".to_owned());
    let authorized_names: Vec<&str> = authorized.iter().map(String::as_str).collect();
    input.authorized_tools = names(&authorized_names);
    input.excluded_tools = excluded_names(Some(&excluded));
    let (sender, _receiver) = tokio::sync::mpsc::channel(16);
    let result = runner
        .run(input, sender, tokio_util::sync::CancellationToken::new())
        .await
        .expect("turn");
    assert_eq!(result.tools_used, Vec::<String>::new());
    assert_eq!(result.response, "kept");
    let requests = provider.requests.lock().clone();
    let first = requests.first().expect("model request");
    let offered: Vec<String> = first
        .tools
        .iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(offered.iter().any(|name| name == "echo"));
    for name in &excluded {
        assert!(!offered.iter().any(|offered| offered == name));
    }
    harness.close().await;
}
