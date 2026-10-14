use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Skip {
    file: String,
    id: String,
    reason: String,
}

#[test]
fn every_out_of_slice_recording_is_named_with_a_reason() {
    let skips: Vec<Skip> = include_str!("golden/wiki/skips.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let listed: BTreeSet<_> = skips
        .iter()
        .map(|skip| {
            assert!(!skip.reason.trim().is_empty(), "{}/{}", skip.file, skip.id);
            (skip.file.clone(), skip.id.clone())
        })
        .collect();
    assert_eq!(listed.len(), skips.len(), "duplicate skip entry");
    let mut recorded = BTreeSet::new();
    let wire = Path::new(env!("CARGO_MANIFEST_DIR")).join("../wire/tests/golden/gateway");
    for (file, path) in [
        (
            "http-plain.json",
            testkit::recording("gateway/http-plain.json"),
        ),
        ("http-tls.json", wire.join("http-tls.json")),
    ] {
        for case in testkit::http::cases(&path).unwrap() {
            if let Some(head) = testkit::http::parse_head(&case.request.head) {
                assert!(
                    !head.target.starts_with("/api/wiki") && !head.target.starts_with("/wiki"),
                    "wiki case must be replayed rather than skipped: {file}/{}",
                    case.id,
                );
            }
            recorded.insert((file.to_owned(), case.id));
        }
    }
    for step in testkit::ws::steps(&wire.join("ws.json")).unwrap() {
        recorded.insert(("ws.json".to_owned(), step.id));
    }
    assert_eq!(
        listed, recorded,
        "recording scope changed: review and replay any wiki cases"
    );
}
