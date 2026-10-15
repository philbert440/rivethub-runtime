#[path = "common/mod.rs"]
#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use common::captured_requests;
use common::chat_provider;
use common::drive;
use common::scenario_record;
use common::scenario_server;
use kernel::model::ModelProvider;
use testkit::Json;
use testkit::ProviderTurn;

const REPLAYED_SCENARIOS: &[&str] = &[
    "A-text",
    "B-tool",
    "C-reasoning",
    "D-thinking-off",
    "E-reasoning-tool",
    "probe",
];

const SKIPPED_SCENARIOS: &[(&str, &str)] = &[];

#[tokio::test]
async fn a_text_replays_against_the_recording() {
    drive("A-text").await;
}

#[tokio::test]
async fn b_tool_replays_against_the_recording() {
    drive("B-tool").await;
}

#[tokio::test]
async fn c_reasoning_replays_against_the_recording() {
    drive("C-reasoning").await;
}

#[tokio::test]
async fn d_thinking_off_replays_against_the_recording() {
    drive("D-thinking-off").await;
}

#[tokio::test]
async fn e_reasoning_tool_replays_against_the_recording() {
    drive("E-reasoning-tool").await;
}

#[tokio::test]
async fn probe_replays_against_the_recording() {
    let recorded = scenario_record("probe");
    let server = scenario_server(recorded.canned.clone()).await;
    let provider = chat_provider(server.addr, None);
    let models = provider
        .list_models()
        .await
        .expect("the probe lists models");
    assert_eq!(models, vec!["test-model".to_owned()]);
    let turn = ProviderTurn {
        requests: captured_requests(&server.recorder),
        chunks: Vec::new(),
    };
    drop(server);
    common::replay(&recorded, &turn);
}

#[tokio::test]
async fn a_configured_max_tokens_is_sent_as_is() {
    let recorded = scenario_record("A-text");
    let raw_recorded =
        testkit::provider::load_scenario(&testkit::recording("providers/vllm/A-text"))
            .expect("the recorded scenario loads");
    let server = scenario_server(recorded.canned.clone()).await;
    let provider = chat_provider(server.addr, Some(4096));
    let raw = raw_recorded
        .requests
        .first()
        .expect("the recorded request")
        .body
        .clone();
    let body: Json = serde_json::from_str(&raw).expect("body parses");
    let mut rx = common::model_request_to_stream(&provider, "A-text", &body)
        .await
        .expect("the stream starts");
    while rx.recv().await.is_some() {}
    let call = server
        .recorder
        .lock()
        .first()
        .cloned()
        .expect("the chat request is captured");
    let sent: Json = serde_json::from_str(&call.body).expect("sent body parses");
    assert_eq!(
        sent.get("max_tokens")
            .and_then(Json::as_u64)
            .expect("configured max_tokens travels"),
        4096
    );
}

#[test]
fn every_recorded_scenario_is_replayed_or_skipped() {
    let root = testkit::recording("providers/vllm");
    let mut recorded = Vec::new();
    for entry in std::fs::read_dir(&root).expect("the vllm recordings are on disk") {
        let entry = entry.expect("a recording entry reads");
        if !entry.file_type().expect("the entry type").is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            panic!("a recording name is not utf-8");
        };
        recorded.push(name.to_owned());
    }
    recorded.sort();
    let mut skipped = Vec::new();
    for (name, reason) in SKIPPED_SCENARIOS {
        assert!(!reason.is_empty(), "{name} needs a reason");
        assert!(root.join(name).is_dir(), "{name} is not a recording");
        assert!(
            !REPLAYED_SCENARIOS.contains(name),
            "{name} is both replayed and skipped"
        );
        skipped.push((*name).to_owned());
    }
    let mut unique = skipped.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), skipped.len(), "skipped names repeat");
    let mut expected: Vec<String> = REPLAYED_SCENARIOS
        .iter()
        .map(|name| (*name).to_owned())
        .chain(skipped)
        .collect();
    expected.sort();
    assert_eq!(recorded, expected);
}
