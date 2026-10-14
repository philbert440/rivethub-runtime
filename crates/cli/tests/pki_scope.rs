#[derive(Debug, PartialEq, Eq, serde::Deserialize)]
struct Skipped {
    name: String,
    reason: String,
}

#[derive(Debug, serde::Deserialize)]
struct Scope {
    contract: std::path::PathBuf,
    skipped: Vec<Skipped>,
}

#[test]
fn no_cli_pki_design_scenarios_are_skipped() -> Result<(), Box<dyn std::error::Error>> {
    let scope: Scope = serde_json::from_str(&std::fs::read_to_string(testkit::golden_path(
        env!("CARGO_MANIFEST_DIR"),
        "tests/fixtures/pki/skipped.json",
    ))?)?;
    assert_eq!(scope.contract, std::path::Path::new("docs/PKI.md"));
    assert_eq!(scope.skipped, Vec::<Skipped>::new());
    Ok(())
}
