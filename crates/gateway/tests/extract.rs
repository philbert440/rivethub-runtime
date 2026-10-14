#![cfg(test)]
#[expect(
    dead_code,
    reason = "the harness module is included whole by every test binary and this one uses a subset"
)]
mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::error::Category;
use testkit::http::HttpCase;
use wire::JsonObject;
use wire::events::AgentEvent;
use wire::gateway::GatewayError;

const REPLAYED: &[&str] = &[
    "plain-main/event-empty-body",
    "plain-main/event-invalid-json",
    "plain-main/layout-post-invalid-json",
];

const SKIPPED: &[(&str, &str)] = &[(
    "plain-main/event-json-string",
    "B2: a JSON string is a typed object-shape error (body must be a JSON object), not the legacy collapsed 422 not a valid v1 AgentEvent",
)];

#[test]
fn recorded_bad_json_bodies_match() {
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let handle = runtime.handle().clone();
    let cases = load_cases();
    let app = app();
    let masks = masks();
    for id in REPLAYED {
        let case = cases
            .iter()
            .find(|case| case.id == *id)
            .unwrap_or_else(|| panic!("missing recording {id}"));
        let app = app.clone();
        let outcome = testkit::http::replay(case, &masks, |request| {
            let answered = handle.block_on(common::answer(app.clone(), request.clone()));
            match answered {
                Ok(response) => response,
                Err(error) => testkit::Response::json(599, &error),
            }
        });
        if let Err(mismatch) = outcome {
            panic!("{}: {}", mismatch.id, mismatch.detail);
        }
    }
}

#[tokio::test]
async fn recorded_non_json_without_an_object_prefix_stays_invalid_json() {
    let cases = load_cases();
    let recorded = cases
        .iter()
        .find(|case| case.id == "plain-main/layout-post-invalid-json")
        .expect("recorded non-json body");
    let body = recorded
        .request
        .body
        .clone()
        .expect("recorded request body");
    assert_eq!(recorded_message(recorded).as_deref(), Some("invalid JSON"));
    let response = post_event(&body).await;
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "invalid JSON");
}

#[tokio::test]
async fn recorded_non_object_json_is_an_object_error() {
    let cases = load_cases();
    let recorded = cases
        .iter()
        .find(|case| case.id == "plain-main/event-json-string")
        .expect("recorded non-object body");
    let body = recorded
        .request
        .body
        .clone()
        .expect("recorded request body");
    let shape = serde_json::from_slice::<JsonObject>(body.as_bytes()).expect_err("not an object");
    assert_eq!(shape.classify(), Category::Data);
    let response = post_event(&body).await;
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "body must be a JSON object");
}

#[tokio::test]
async fn recorded_object_field_errors_come_from_the_typed_parse() {
    let cases = load_cases();
    let recorded = cases
        .iter()
        .find(|case| case.id == "plain-main/event-bad-ts")
        .expect("recorded object with a bad field");
    let body = recorded
        .request
        .body
        .clone()
        .expect("recorded request body");
    let typed = serde_json::from_str::<AgentEvent>(&body).expect_err("field type");
    assert_eq!(typed.classify(), Category::Data);
    let response = post_event(&body).await;
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, typed.to_string());
}

#[test]
fn recorded_bad_json_cases_are_replayed_or_skipped_by_name() {
    let cases = load_cases();
    let slice: BTreeSet<&str> = cases
        .iter()
        .filter(|case| in_slice(case))
        .map(|case| case.id.as_str())
        .collect();
    let mut accounted = BTreeSet::new();
    for id in REPLAYED {
        assert!(
            slice.contains(id),
            "replay is outside the bad-JSON cases: {id}"
        );
        assert!(accounted.insert(*id), "duplicate replay {id}");
    }
    for (id, reason) in SKIPPED {
        assert!(!reason.trim().is_empty(), "empty skip reason for {id}");
        assert!(
            slice.contains(id),
            "skip is outside the bad-JSON cases: {id}"
        );
        assert!(accounted.insert(*id), "duplicate skip {id}");
    }
    assert_eq!(slice, accounted);
}

fn in_slice(case: &HttpCase) -> bool {
    recorded_message(case).as_deref() == Some("invalid JSON") || non_object_event(case)
}

fn non_object_event(case: &HttpCase) -> bool {
    let Some(head) = testkit::http::parse_head(&case.request.head) else {
        return false;
    };
    let path = match head.target.split_once('?') {
        Some((path, _)) => path,
        None => head.target.as_str(),
    };
    if head.method != "POST" || !matches!(path, "/event" | "/api/events/event") {
        return false;
    }
    let Some(body) = case.request.body.as_deref() else {
        return false;
    };
    match serde_json::from_slice::<JsonObject>(body.as_bytes()) {
        Err(error) => error.classify() == Category::Data,
        Ok(_) => false,
    }
}

fn recorded_message(case: &HttpCase) -> Option<String> {
    let text = case.response.body.as_deref()?;
    let parsed: GatewayError = serde_json::from_str(text).ok()?;
    Some(parsed.error)
}

fn load_cases() -> Vec<HttpCase> {
    testkit::http::cases(&testkit::recording("gateway/http-plain.json")).expect("gateway recording")
}

fn masks() -> testkit::Masks {
    testkit::Masks {
        port: "5174".to_owned(),
        hostname: Some("gold-host".to_owned()),
        tmp: Some("/tmp/golden".to_owned()),
    }
}

fn app() -> axum::Router {
    let tenancy = gateway::tenancy::Tenancy::local(
        gateway::tenancy::UserId::new("owner".to_owned()).expect("owner"),
    );
    let gateway = Arc::new(
        gateway::Gateway::new(
            Arc::new(testkit::FakeMemoryStore::default()),
            common::settings(),
        )
        .with_tenancy(tenancy),
    );
    gateway.router(false)
}

async fn post_event(body: &str) -> testkit::Response {
    common::answer(
        app(),
        testkit::Request {
            method: "POST".to_owned(),
            target: "/event".to_owned(),
            headers: vec![
                ("Host".to_owned(), "127.0.0.1:5174".to_owned()),
                ("Content-Type".to_owned(), "application/json".to_owned()),
            ],
            body: body.as_bytes().to_vec(),
            recipe: None,
            via: Some("loopback (source 127.0.0.1)".to_owned()),
        },
    )
    .await
    .expect("in-process answer")
}

fn error_of(response: &testkit::Response) -> GatewayError {
    serde_json::from_slice(&response.body).expect("error response")
}
