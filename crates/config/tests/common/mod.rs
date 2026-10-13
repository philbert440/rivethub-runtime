use std::path::{Path, PathBuf};

use similar_asserts::SimpleDiff;

pub(crate) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("fixtures")
        .join(format!("{name}.jsonl"))
}

pub(crate) fn read_cases(name: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(fixture(name))?;
    Ok(text
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

pub(crate) fn parse_case<T: serde::de::DeserializeOwned>(
    name: &str,
    index: usize,
    line: &str,
) -> Result<T, Box<dyn std::error::Error>> {
    let label = format!("{name} case {}", index + 1);
    serde_json::from_str(line).map_err(|error| format!("parse {label}: {error}").into())
}

pub(crate) fn case_id(name: &str, index: usize) -> String {
    format!("{name}.jsonl:{}", index + 1)
}

pub(crate) fn json_text(value: &impl serde::Serialize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|error| format!("<unserializable: {error}>"))
}

pub(crate) fn case_failure(
    id: &str,
    reason: &str,
    input: &impl serde::Serialize,
    expected: &str,
    actual: &str,
) -> String {
    format!(
        "=== case {id} — {reason} ===\ninput:\n{}\nexpected:\n{expected}\nactual:\n{actual}\n{}",
        json_text(input),
        SimpleDiff::from_str(expected, actual, "expected", "actual")
    )
}
