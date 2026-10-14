use std::collections::BTreeSet;
use std::path::PathBuf;

use gateway::mesh::Registry;
use serde::Deserialize;
use wire::mesh::MeshParseCode;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const REJECTIONS: &[(&str, &str)] = &[
    ("plain-main/mesh-root-array", "[]"),
    ("plain-main/mesh-flat-array", r#"{"nodes":[]}"#),
    ("plain-main/mesh-nodes-number", r#"{"nodes":42}"#),
];

const ABSENCES: &[&str] = &[
    "plain-main/mesh-no-file",
    "plain-redirect/mesh-default-path-missing",
];

const SKIPPED: &[(&str, &str)] = &[
    (
        "plain-main/mesh-valid-before-ingest",
        "B1 overview enrichment belongs to the HTTP routes lane; the raw registry fixture is absent",
    ),
    (
        "plain-main/mesh-valid",
        "B1 overview enrichment and session counts belong to the HTTP routes lane",
    ),
    (
        "plain-main/alias-api-mesh",
        "B1 HTTP route alias belongs to the routes lane",
    ),
    (
        "plain-main/mesh-post",
        "B1 HTTP method routing belongs to the routes lane",
    ),
    (
        "plain-main/mesh-empty-object",
        "B14 writes require a versioned registry; an empty object is a typed error, not an implicit empty registry",
    ),
    (
        "plain-main/mesh-invalid-json",
        "B14 data integrity: corrupt JSON is an explicit error, never swallowed as absence",
    ),
    (
        "plain-main/origin-mesh-peer-flipped-scheme",
        "B1 browser origin policy belongs to the HTTP routes lane",
    ),
    (
        "plain-main/origin-mesh-peer-explicit-scheme-only",
        "B1 browser origin policy belongs to the HTTP routes lane",
    ),
    (
        "plain-nostatic/capabilities-no-mesh",
        "B1 subsystem capabilities belong to the HTTP routes lane",
    ),
];

#[derive(Deserialize)]
struct RecordedError {
    error: String,
    code: Option<MeshParseCode>,
}

#[tokio::test]
async fn recorded_mesh_parse_errors_and_absence_replay_through_registry() -> TestResult {
    let cases = testkit::http::cases(&testkit::recording("gateway/http-plain.json"))?;
    let root = tempfile::tempdir()?;
    let path = root.path().join("mesh.json");
    let registry = Registry::open(root.path().to_owned(), PathBuf::from("mesh.json")).await?;
    for id in ABSENCES {
        let case = cases
            .iter()
            .find(|case| case.id == *id)
            .ok_or("missing recorded absence")?;
        let recorded: RecordedError =
            serde_json::from_str(case.response.body.as_deref().ok_or("missing response")?)?;
        assert_eq!(recorded.error, "no mesh file");
        assert_eq!(registry.snapshot().await?, None);
    }
    for (id, input) in REJECTIONS {
        let case = cases
            .iter()
            .find(|case| case.id == *id)
            .ok_or("missing recorded parse rejection")?;
        let recorded: RecordedError =
            serde_json::from_str(case.response.body.as_deref().ok_or("missing response")?)?;
        std::fs::write(&path, input)?;
        let actual = registry
            .snapshot()
            .await
            .expect_err("invalid registry must fail");
        assert_eq!(actual.parse_code(), recorded.code, "{id}");
    }
    Ok(())
}

#[test]
fn every_recorded_mesh_case_is_replayed_or_skipped_by_name_with_a_reason() -> TestResult {
    let cases = testkit::http::cases(&testkit::recording("gateway/http-plain.json"))?;
    let recorded: BTreeSet<_> = cases
        .iter()
        .filter(|case| case.id.contains("mesh"))
        .map(|case| case.id.as_str())
        .collect();
    let mut accounted = BTreeSet::new();
    for (id, _) in REJECTIONS {
        assert!(accounted.insert(*id));
    }
    for id in ABSENCES {
        assert!(accounted.insert(*id));
    }
    for (id, reason) in SKIPPED {
        assert_ne!(reason.trim(), "");
        assert!(accounted.insert(*id));
    }
    assert_eq!(recorded, accounted);
    Ok(())
}
