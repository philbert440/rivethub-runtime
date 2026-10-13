use testkit::Json;
use testkit::json_eq;

#[test]
fn batch_validation_matches_recorded_outcomes() -> Result<(), Box<dyn std::error::Error>> {
    let path = testkit::recording("capture/capture-zod/capture-batch.json");
    let text = std::fs::read_to_string(&path)?;
    let recording: Json = serde_json::from_str(&text)?;
    let cases = recording
        .get("cases")
        .and_then(testkit::Json::as_object)
        .ok_or("cases missing")?;
    let mut failures = Vec::new();
    for (name, case) in cases {
        let raw = serde_json::to_string(&case["input"])?;
        let expected_ok = case
            .get("ok")
            .and_then(testkit::Json::as_bool)
            .ok_or("case without ok")?;
        match capture::batch::parse_batch(&raw) {
            Ok(batch) => {
                if !expected_ok {
                    failures.push(format!("{name}: accepted, recorded rejection"));
                    continue;
                }
                let Some(expected) = case.get("parsed") else {
                    continue;
                };
                let actual = serde_json::to_value(&batch)?;
                if !json_eq(expected, &actual) {
                    failures.push(format!(
                        "{name}: parsed batch mismatch\n    recorded: {expected}\n    actual:   {actual}"
                    ));
                }
            }
            Err(error) => {
                if expected_ok {
                    failures.push(format!("{name}: rejected: {error}"));
                }
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} validation failure(s):\n{}",
            failures.len(),
            failures.join("\n")
        )
        .into())
    }
}
