use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Skipped {
    name: String,
    reason: String,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize)]
struct Case {
    id: String,
}

const REPLAYED: [&str; 4] = [
    "sign-with-token",
    "sign-token-reused",
    "sign-san-not-granted",
    "roots",
];

const SAN_POLICY: [&str; 15] = [
    "node_valid",
    "agent_valid",
    "device_valid",
    "node_impersonation",
    "kind_escalation",
    "agent_cross_node",
    "agent_cross_agent",
    "device_cross_user",
    "device_cross_device",
    "foreign_trust_domain",
    "additional_identity",
    "duplicate_san",
    "encoded_separator",
    "uri_query",
    "empty_san",
];

#[test]
fn out_of_slice_scenarios_are_named_and_explained() -> Result<(), Box<dyn std::error::Error>> {
    let entries: Vec<Skipped> = serde_json::from_str(include_str!("fixtures/skipped.json"))?;
    let fixture: Fixture = serde_json::from_str(&std::fs::read_to_string(testkit::recording(
        "stepca/api.json",
    ))?)?;
    let mut expected = REPLAYED
        .into_iter()
        .map(str::to_owned)
        .chain(SAN_POLICY.into_iter().map(str::to_owned))
        .collect::<Vec<_>>();
    expected.extend(entries.iter().map(|entry| entry.name.clone()));
    expected.sort();
    expected.dedup();
    let mut recorded = fixture
        .cases
        .iter()
        .map(|case| case.id.clone())
        .chain(SAN_POLICY.into_iter().map(str::to_owned))
        .collect::<Vec<_>>();
    recorded.sort();
    assert_eq!(expected, recorded);
    for entry in &entries {
        assert!(entry.reason.len() >= 60, "{}", entry.name);
        assert!(!REPLAYED.contains(&entry.name.as_str()), "{}", entry.name);
    }
    let skipped = entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect::<Vec<_>>();
    for id in fixture.cases.iter().map(|case| case.id.as_str()) {
        assert!(
            REPLAYED.contains(&id) || skipped.contains(&id),
            "{id} is neither replayed nor skipped"
        );
    }
    Ok(())
}
