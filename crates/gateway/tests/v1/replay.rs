use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use testkit::http::HttpCase;
use wire::gateway::GatewayError;

use super::common;

const REPLAYED: [&str; 3] = [
    "plain-main/options-api",
    "plain-main/options-api-disallowed-origin",
    "plain-main/not-found-api",
];

#[derive(Debug, Deserialize)]
struct Skip {
    recording: String,
    id: String,
    reason: String,
}

fn golden(name: &str) -> std::path::PathBuf {
    testkit::golden_path(env!("CARGO_MANIFEST_DIR"), "../wire/tests/golden/gateway").join(name)
}

fn http_cases() -> BTreeMap<String, Vec<HttpCase>> {
    [
        (
            "http-plain.json",
            testkit::recording("gateway/http-plain.json"),
        ),
        ("http-tls.json", golden("http-tls.json")),
    ]
    .into_iter()
    .map(|(name, path)| {
        (
            name.to_owned(),
            testkit::http::cases(&path).expect("HTTP recording"),
        )
    })
    .collect()
}

#[test]
fn every_recorded_scenario_is_replayed_or_named_with_a_reason() {
    let skips: Vec<Skip> =
        serde_json::from_str(include_str!("../fixtures/v1/skips.json")).expect("skip list");
    let cases = http_cases();
    let mut recorded = BTreeSet::new();
    for (name, cases) in &cases {
        for case in cases {
            let head = testkit::http::parse_head(&case.request.head);
            if let Some(head) = head {
                assert!(
                    !head.target.starts_with("/v1/")
                        && !head.target.starts_with("/api/conversations"),
                    "new in-slice recording must be replayed: {}",
                    case.id
                );
            }
            assert!(recorded.insert((name.clone(), case.id.clone())));
        }
    }
    let ws: testkit::ws::Recording =
        serde_json::from_str(&std::fs::read_to_string(golden("ws.json")).expect("WS fixture"))
            .expect("WS recording");
    for (run, entry) in ws.runs {
        for step in entry.steps {
            assert!(recorded.insert(("ws.json".to_owned(), format!("{run}/{}", step.id))));
        }
    }
    let mut accounted = BTreeSet::new();
    for id in REPLAYED {
        assert!(accounted.insert(("http-plain.json".to_owned(), id.to_owned())));
    }
    for skip in skips {
        assert_ne!(skip.reason.trim(), "");
        assert!(
            accounted.insert((skip.recording, skip.id)),
            "duplicate or replayed skip"
        );
    }
    assert_eq!(recorded, accounted);
    let users = std::fs::read_to_string(golden("users-tenancy.json")).expect("tenant fixture");
    assert!(serde_json::from_str::<wire::gateway::users::UsersRegistry>(&users).is_ok());
    let invalid = std::fs::read_to_string(golden("users-invalid.json")).expect("invalid fixture");
    assert!(serde_json::from_str::<wire::gateway::users::UsersRegistry>(&invalid).is_err());
}

#[tokio::test]
async fn existing_preflight_and_not_found_recordings_survive_the_mount() {
    let masks = testkit::Masks {
        port: "5174".to_owned(),
        hostname: Some("gold-host".to_owned()),
        tmp: Some("/tmp/golden".to_owned()),
    };
    let cases = http_cases();
    let (gateway, _) = common::gateway();
    for case in cases
        .get("http-plain.json")
        .expect("plain cases")
        .iter()
        .filter(|case| REPLAYED.contains(&case.id.as_str()))
    {
        let head = testkit::http::parse_head(&testkit::substitute(&case.request.head, &masks))
            .expect("recorded head");
        let actual = common::answer(
            gateway.router(false),
            testkit::Request {
                method: head.method,
                target: head.target,
                headers: head.headers,
                body: case
                    .request
                    .body
                    .as_ref()
                    .map_or_else(Vec::new, |body| body.as_bytes().to_vec()),
                recipe: None,
                via: case.request.via.clone(),
            },
        )
        .await
        .expect("HTTP replay");
        assert_eq!(
            actual.status,
            testkit::http::status_of(&case.response.status_line),
            "{}",
            case.id
        );
        if actual.body.is_empty() {
            assert!(case.response.body.as_deref().is_none_or(str::is_empty));
        } else {
            let actual: GatewayError =
                serde_json::from_slice(&actual.body).expect("typed actual error");
            let expected: GatewayError =
                serde_json::from_str(case.response.body.as_deref().expect("expected body"))
                    .expect("typed recorded error");
            assert_eq!(actual, expected, "{}", case.id);
        }
    }
}
