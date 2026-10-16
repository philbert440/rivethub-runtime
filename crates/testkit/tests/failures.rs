use std::collections::BTreeMap;

use kernel::model::{ModelProvider, ModelRequest};
use kernel::tool::Tool;
use kernel::transcript::TranscriptReader;
use testkit::{FakeProvider, FakeReader, FakeTool, Json, Masks, Response};

fn request() -> ModelRequest {
    ModelRequest {
        options: kernel::model::ModelOptions::default(),
        model: "test".to_owned(),
        messages: Vec::new(),
        tools: Vec::new(),
        thinking: None,
        max_tokens: None,
    }
}

#[tokio::test]
async fn missing_provider_scripts_are_errors() {
    let provider = FakeProvider::default();
    assert!(provider.chat(request()).await.is_err());
    assert!(provider.stream(request()).await.is_err());
    assert!(provider.embed(vec!["text".to_owned()]).await.is_err());
    assert!(provider.list_models().await.is_err());
    assert!(
        FakeProvider::fnv_embeddings(0)
            .embed(vec!["text".to_owned()])
            .await
            .is_err()
    );
}

#[tokio::test]
async fn reader_refuses_invalid_keys_and_missing_sessions() {
    assert!(FakeReader::new(BTreeMap::from([("invalid".to_owned(), Vec::new())])).is_err());
    let session = "claude-code:example".parse().expect("session id");
    let reader = FakeReader::new(BTreeMap::new()).expect("empty fixture");
    assert!(reader.read(&session).await.is_err());
    let reader = FakeReader::new(BTreeMap::from([(
        "claude-code:example".to_owned(),
        Vec::new(),
    )]))
    .expect("valid fixture");
    assert_eq!(
        reader
            .read(&session)
            .await
            .expect("explicit empty transcript"),
        Vec::new()
    );
}

#[test]
fn tool_schema_is_derived_from_the_echo_input() {
    let tool = FakeTool::new("echo").expect("schema");
    let schema = tool.definition().input_schema;
    assert_eq!(schema.get("required"), Some(&serde_json::json!(["text"])));
    assert_eq!(
        schema
            .get("properties")
            .and_then(|value| value.get("text"))
            .and_then(|value| value.get("type")),
        Some(&Json::String("string".to_owned()))
    );
}

#[test]
fn invalid_http_status_and_base64_are_errors() {
    assert!(testkit::http::status_of("invalid").is_err());
    assert_eq!(
        testkit::http::status_of("HTTP/1.1 302 Found").expect("redirect"),
        302
    );
    let mut case = testkit::http::HttpCase {
        id: "invalid-base64".to_owned(),
        request: testkit::http::RequestRecord {
            head: "GET / HTTP/1.1\r\nHost: example.test\r\n\r\n".to_owned(),
            ..Default::default()
        },
        response: testkit::http::ResponseRecord {
            status_line: "HTTP/1.1 200 OK".to_owned(),
            body_base64: Some("invalid!".to_owned()),
            ..Default::default()
        },
    };
    assert!(
        testkit::http::replay(&case, &Masks::default(), |_| Response::bytes(
            200,
            "application/octet-stream",
            []
        ))
        .is_err()
    );
    case.response.body = Some("text".to_owned());
    assert!(
        testkit::http::replay(&case, &Masks::default(), |_| Response::bytes(
            200,
            "text/plain",
            "text"
        ))
        .is_err()
    );
}

#[test]
fn invalid_sqlite_text_is_an_error() {
    let connection = rusqlite::Connection::open_in_memory().expect("isolated database");
    assert!(testkit::query_json(&connection, "SELECT CAST(x'ff' AS TEXT)", &[]).is_err());
}

#[test]
fn invalid_recorded_unicode_is_not_repaired() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let path = directory.path().join("capture.json");
    std::fs::write(&path, r#"[{"stdin":"\ud800"}]"#).expect("write fixture");
    assert!(testkit::capture::entries(&path).is_err());
    std::fs::write(&path, r#"[{"stdin":"\ud83d\ude00"}]"#).expect("write fixture");
    assert_eq!(
        testkit::capture::entries(&path)
            .expect("valid pair")
            .first()
            .expect("one entry")
            .stdin,
        "😀"
    );
}

#[test]
fn postgres_replay_does_not_round_large_integers() {
    let scenario = testkit::capture_pg::ScenarioRecord {
        name: "integer-integrity".to_owned(),
        steps: vec![testkit::capture_pg::StepRecord {
            rows: Some(serde_json::json!({"count": 9_007_199_254_740_992_u64})),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(
        testkit::capture_pg::replay_scenario(&scenario, None, |_| testkit::PgOutcome {
            rows: Some(serde_json::json!({"count": 9_007_199_254_740_993_u64})),
            ..Default::default()
        })
        .is_err()
    );
}

#[test]
fn bodyless_http_responses_preserve_protocol_semantics() {
    for (method, status) in [("HEAD", 200), ("GET", 204), ("GET", 205), ("GET", 304)] {
        let case = testkit::http::HttpCase {
            id: "bodyless".to_owned(),
            request: testkit::http::RequestRecord {
                head: format!("{method} / HTTP/1.1\r\nHost: example.test\r\n\r\n"),
                ..Default::default()
            },
            response: testkit::http::ResponseRecord {
                status_line: format!("HTTP/1.1 {status} Test"),
                headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
                ..Default::default()
            },
        };
        assert!(
            testkit::http::replay(&case, &Masks::default(), |_| {
                Response::bytes(status, "application/json", [])
            })
            .is_ok()
        );
        assert!(
            testkit::http::replay(&case, &Masks::default(), |_| {
                Response::bytes(status, "application/json", b"{}")
            })
            .is_err()
        );
    }
}

#[test]
fn identical_invalid_json_bodies_are_refused() {
    for body in ["", "invalid"] {
        let case = testkit::http::HttpCase {
            id: "invalid-json".to_owned(),
            request: testkit::http::RequestRecord {
                head: "GET / HTTP/1.1\r\nHost: example.test\r\n\r\n".to_owned(),
                ..Default::default()
            },
            response: testkit::http::ResponseRecord {
                status_line: "HTTP/1.1 200 OK".to_owned(),
                headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
                body: Some(body.to_owned()),
                ..Default::default()
            },
        };
        assert!(
            testkit::http::replay(&case, &Masks::default(), |_| {
                Response::bytes(200, "application/json", body)
            })
            .is_err()
        );
    }
}
