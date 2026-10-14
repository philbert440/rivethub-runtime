#[path = "common/mod.rs"]
mod common;

use common::generic_provider;
use common::scenario_record;
use common::scenario_server;
use kernel::model::ModelProvider;
use providers::body::normalize_api_prefix;
use providers::body::split_and_fold_system;
use serde::Deserialize;
use serde::Serialize;
use testkit::Json;
use testkit::Thrown;
use testkit::WireDelta;
use wire::message::ContentPart;
use wire::message::Role;

#[expect(
    clippy::expect_used,
    reason = "test helper; the clippy.toml exemption only reaches #[test] functions"
)]
fn vectors(rel: &str) -> Vec<testkit::cases::DiffCase> {
    let path = testkit::recording(rel);
    testkit::cases::cases(&path).expect("the diff vectors load")
}

#[tokio::test]
async fn normalize_api_prefix_matches_the_recorded_vectors() {
    let cases = vectors("providers/diff/normalize_api_prefix.jsonl");
    assert_eq!(cases.len(), 13);
    for case in &cases {
        testkit::cases::replay(case, |function, input| {
            if function != "normalize_api_prefix" {
                return Err(Thrown("unexpected function".to_owned()));
            }
            let arg = match input {
                Json::String(text) => Some(text.as_str()),
                Json::Null | Json::Bool(_) | Json::Number(_) | Json::Array(_) | Json::Object(_) => {
                    None
                }
            };
            Ok(Json::String(normalize_api_prefix(arg)))
        })
        .expect("the prefix vector replays");
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct VmMessage {
    role: Role,
    content: VmContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<VmToolCall>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
enum VmContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
struct VmToolCall {
    id: String,
    name: String,
    arguments: VmArguments,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thought_signature: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
enum VmArguments {
    Text(String),
    Object(wire::JsonObject),
}

impl VmMessage {
    fn text(&self) -> String {
        match &self.content {
            VmContent::Text(text) => text.clone(),
            VmContent::Parts(parts) => parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::Image(_) | ContentPart::Video(_) => None,
                })
                .collect(),
        }
    }

    fn notice(text: String) -> Self {
        Self {
            role: Role::User,
            content: VmContent::Text(text),
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

#[derive(Serialize)]
struct Folded {
    system: String,
    rest: Vec<VmMessage>,
}

#[tokio::test]
async fn split_and_fold_system_matches_the_recorded_vectors() {
    let cases = vectors("providers/diff/split_and_fold_system.jsonl");
    assert_eq!(cases.len(), 253);
    for case in &cases {
        testkit::cases::replay(case, |function, input| {
            if function != "split_and_fold_system" {
                return Err(Thrown("unexpected function".to_owned()));
            }
            let messages: Vec<VmMessage> = serde_json::from_value(input.clone())
                .map_err(|error| Thrown(format!("undecodable messages: {error}")))?;
            let (system, rest) = split_and_fold_system(
                messages,
                |message| message.role,
                VmMessage::text,
                VmMessage::notice,
            );
            let folded = Folded { system, rest };
            serde_json::to_value(&folded).map_err(|error| Thrown(error.to_string()))
        })
        .expect("the fold vector replays");
    }
}

#[tokio::test]
async fn the_recorded_request_carries_exactly_the_table_fields() {
    let table = testkit::provider::B8_OMITTED_REQUEST_FIELDS;
    assert_eq!(
        table,
        &[
            ("temperature", WireDelta::SamplingOnModelServer),
            ("top_p", WireDelta::SamplingOnModelServer),
            ("max_tokens", WireDelta::OutputLimitOnModelServer),
            ("tool_choice", WireDelta::ServerDefaultToolChoice),
        ]
    );
    let raw_scenario =
        testkit::provider::load_scenario(&testkit::recording("providers/vllm/A-text"))
            .expect("the recorded scenario loads");
    let raw = raw_scenario
        .requests
        .first()
        .expect("the recorded request")
        .body
        .clone();
    let parsed: Json = serde_json::from_str(&raw).expect("recorded body parses");
    for (field, _) in table {
        assert!(
            parsed.get(field).is_some(),
            "the recorded body lost {field} before the table saw it"
        );
    }
    let stripped = testkit::strip_b8_deltas(&raw).expect("the recorded body strips");
    let stripped_json: Json = serde_json::from_str(&stripped).expect("stripped body parses");
    for (field, _) in table {
        assert!(
            stripped_json.get(field).is_none(),
            "{field} survived the strip"
        );
    }
    assert_eq!(stripped_json.get("stream"), Some(&Json::Bool(true)));
}

#[tokio::test]
async fn api_prefix_builds_the_request_target() {
    let recorded = scenario_record("A-text");
    let server = scenario_server(recorded.canned.clone()).await;
    let provider = config::provider::OpenaiProvider {
        model: Some("test-model".to_owned()),
        base_url: Some(format!("http://{}/", server.addr)),
        api_prefix: Some("openai/v1".to_owned()),
        ..config::provider::OpenaiProvider::default()
    };
    let built = providers::OpenAiCompatible::build(
        config::provider::ProviderType::OpenaiCompatible,
        &provider,
    )
    .expect("the generic provider builds");
    let body: Json = serde_json::from_str(&recorded.requests.first().expect("request").body)
        .expect("body parses");
    let mut rx = built
        .stream(common::model_request("A-text", &body))
        .await
        .expect("the stream starts");
    while rx.recv().await.is_some() {}
    let call = server
        .recorder
        .lock()
        .first()
        .cloned()
        .expect("the chat request is captured");
    assert_eq!(call.target, "/openai/v1/chat/completions");
    let sent: Json = serde_json::from_str(&call.body).expect("sent body parses");
    assert!(
        sent.get("chat_template_kwargs").is_none(),
        "the generic variant sends no vllm thinking flag"
    );
    assert!(
        sent.get("tool_choice").is_none(),
        "tool_choice is never sent"
    );
}

#[tokio::test]
async fn thinking_off_on_the_generic_variant_sends_no_vllm_flag() {
    let recorded = scenario_record("D-thinking-off");
    let server = scenario_server(recorded.canned.clone()).await;
    let built = generic_provider(server.addr);
    let body: Json = serde_json::from_str(&recorded.requests.first().expect("request").body)
        .expect("body parses");
    let request = common::model_request("D-thinking-off", &body);
    let mut rx = built.stream(request).await.expect("the stream starts");
    while rx.recv().await.is_some() {}
    let call = server
        .recorder
        .lock()
        .first()
        .cloned()
        .expect("the chat request is captured");
    let sent: Json = serde_json::from_str(&call.body).expect("sent body parses");
    assert!(
        sent.get("chat_template_kwargs").is_none(),
        "the thinking flag is a vllm behaviour"
    );
}

#[tokio::test]
async fn a_tool_without_a_description_is_sent_without_one() {
    let recorded = scenario_record("A-text");
    let server = scenario_server(recorded.canned.clone()).await;
    let provider = config::provider::OpenaiProvider {
        model: Some("test-model".to_owned()),
        base_url: Some(format!("http://{}", server.addr)),
        ..config::provider::OpenaiProvider::default()
    };
    let built = providers::OpenAiCompatible::build(
        config::provider::ProviderType::OpenaiCompatible,
        &provider,
    )
    .expect("the generic provider builds");
    let tool: rmcp::model::Tool = serde_json::from_str(
        "{\"name\":\"bare\",\"description\":\"has one\",\"inputSchema\":{\"type\":\"object\",\"properties\":{},\"required\":[]}}",
    )
    .expect("described tool parses");
    let mut undescribed = tool.clone();
    undescribed.description = None;
    let request = kernel::model::ModelRequest {
        model: "test-model".to_owned(),
        messages: Vec::new(),
        tools: vec![undescribed, tool],
        thinking: None,
        max_tokens: None,
    };
    let mut rx = built
        .stream(request)
        .await
        .expect("the undescribed tool is sent");
    while rx.recv().await.is_some() {}
    let call = server
        .recorder
        .lock()
        .first()
        .cloned()
        .expect("the chat request is captured");
    let sent: Json = serde_json::from_str(&call.body).expect("sent body parses");
    let expected: Json = serde_json::from_str(
        "[{\"type\":\"function\",\"function\":{\"name\":\"bare\",\"parameters\":{\"type\":\"object\",\"properties\":{},\"required\":[]}}},\
         {\"type\":\"function\",\"function\":{\"name\":\"bare\",\"description\":\"has one\",\"parameters\":{\"type\":\"object\",\"properties\":{},\"required\":[]}}}]",
    )
    .expect("expected tools parse");
    assert_eq!(sent.get("tools"), Some(&expected));
}

#[tokio::test]
async fn the_request_max_tokens_field_is_not_sent_on_the_wire() {
    let recorded = scenario_record("A-text");
    let server = scenario_server(recorded.canned.clone()).await;
    let provider = config::provider::OpenaiProvider {
        model: Some("test-model".to_owned()),
        base_url: Some(format!("http://{}", server.addr)),
        ..config::provider::OpenaiProvider::default()
    };
    let built = providers::OpenAiCompatible::build(config::provider::ProviderType::Vllm, &provider)
        .expect("the generic provider builds");
    let request = kernel::model::ModelRequest {
        model: "test-model".to_owned(),
        messages: Vec::new(),
        tools: Vec::new(),
        thinking: None,
        max_tokens: Some(999),
    };
    let mut rx = built.stream(request).await.expect("the stream starts");
    while rx.recv().await.is_some() {}
    let call = server
        .recorder
        .lock()
        .first()
        .cloned()
        .expect("the chat request is captured");
    let sent: Json = serde_json::from_str(&call.body).expect("sent body parses");
    assert!(
        sent.get("max_tokens").is_none(),
        "the wire max_tokens comes from the provider config only (CONTRACT decision 11); a caller-set request.max_tokens is not sent: {sent}"
    );
}
