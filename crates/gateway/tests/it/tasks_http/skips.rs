use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Skip {
    id: String,
    reason: String,
}

const REQUIRED: [&str; 4] = [
    "clamp-wait-ms-source-bounds",
    "date-parse-legacy-forms",
    "audit-only-outcome-rows",
    "sqlite-local-queue-node",
];

#[test]
fn skip_list_accounts_for_this_slice() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tasks/skips.json");
    let text = std::fs::read_to_string(path).expect("skip list");
    let skips: Vec<Skip> = serde_json::from_str(&text).expect("skip json");
    let mut ids = BTreeSet::new();
    for skip in &skips {
        assert!(!skip.reason.trim().is_empty(), "{}", skip.id);
        assert!(!skip.reason.contains("rivet-shared"), "{}", skip.id);
        assert!(ids.insert(skip.id.clone()), "duplicate {}", skip.id);
    }
    for id in REQUIRED {
        assert!(ids.contains(id), "{id}");
    }
    let cases =
        testkit::http::cases(&testkit::recording("gateway/http-plain.json")).expect("recording");
    assert!(!cases.is_empty(), "recording has cases");
    let mut parsed = 0_u32;
    for case in &cases {
        let Some(head) = testkit::http::parse_head(&case.request.head) else {
            continue;
        };
        parsed += 1;
        if !in_slice(path_of(&head.target)) {
            continue;
        }
        let raw = match case.id.rsplit_once('/') {
            Some((_, raw)) => raw,
            None => case.id.as_str(),
        };
        assert!(
            ids.contains(&case.id) || ids.contains(raw),
            "in-slice recording {} was not replayed or skipped",
            case.id
        );
    }
    assert!(parsed > 0, "recording heads parse");
}

fn path_of(target: &str) -> &str {
    match target.split('?').next() {
        Some(path) => path,
        None => target,
    }
}

fn in_slice(path: &str) -> bool {
    ["/api/tasks", "/api/catalog", "/api/outcomes"]
        .into_iter()
        .any(|root| path == root || path.starts_with(&format!("{root}/")))
}
