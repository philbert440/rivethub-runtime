use std::path::Path;

use crate::Error;
use crate::Mismatch;
use crate::json::Json;
use crate::json::json_eq;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thrown(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    Out(Json),
    Throws(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffCase {
    pub line: usize,
    pub function: String,
    pub input: Json,
    pub expected: Expected,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputRecord {
    #[serde(rename = "fn")]
    function: String,
    #[serde(rename = "in")]
    input: Json,
    out: Json,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ErrorRecord {
    #[serde(rename = "fn")]
    function: String,
    #[serde(rename = "in")]
    input: Json,
    throws: ThrownRecord,
}

#[derive(serde::Deserialize)]
struct ThrownRecord {
    name: String,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Record {
    Output(OutputRecord),
    Error(ErrorRecord),
}

pub fn cases(path: &Path) -> Result<Vec<DiffCase>, Error> {
    let text = crate::read_text(path)?;
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            let record: Record = serde_json::from_str(line)
                .map_err(|error| Error::format(path, format!("{}: {error}", index + 1)))?;
            let (function, input, expected) = match record {
                Record::Output(row) => (row.function, row.input, Expected::Out(row.out)),
                Record::Error(row) => (row.function, row.input, Expected::Throws(row.throws.name)),
            };
            Ok(DiffCase {
                line: index + 1,
                function,
                input,
                expected,
            })
        })
        .collect()
}

pub fn replay(
    case: &DiffCase,
    classify: impl Fn(&str, &Json) -> Result<Json, Thrown>,
) -> Result<(), Mismatch> {
    let id = format!("{}:{}", case.function, case.line);
    let actual = classify(&case.function, &case.input);
    match (actual, &case.expected) {
        (Ok(value), Expected::Out(expected)) if json_eq(expected, &value) => Ok(()),
        (Ok(value), Expected::Out(expected)) => Err(Mismatch::new(
            &id,
            format!(
                "line {}: expected {expected}, actual {value}\n    input: {}",
                case.line, case.input
            ),
        )),
        (Ok(value), Expected::Throws(_)) => Err(Mismatch::new(
            &id,
            format!(
                "line {}: expected a rejection, actual {value}\n    input: {}",
                case.line, case.input
            ),
        )),
        (Err(_), Expected::Throws(_)) => Ok(()),
        (Err(thrown), Expected::Out(expected)) => Err(Mismatch::new(
            &id,
            format!(
                "line {}: expected {expected}, rejected: {}\n    input: {}",
                case.line, thrown.0, case.input
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_row(name: &str, row: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("a temp dir for the case row");
        let path = dir.path().join(format!("{name}.jsonl"));
        std::fs::write(&path, row).expect("the temp row writes");
        (dir, path)
    }

    #[test]
    fn a_row_without_in_is_a_load_error() {
        let (_dir, path) = write_row(
            "missing-in",
            "{\"fn\":\"normalize_api_prefix\",\"out\":null}\n",
        );
        let error = cases(&path).expect_err("missing in fails the load");
        let text = error.to_string();
        assert!(text.contains("recording"), "{text}");
    }

    #[test]
    fn an_explicit_null_in_still_loads() {
        let (_dir, path) = write_row(
            "null-in",
            "{\"fn\":\"normalize_api_prefix\",\"in\":null,\"out\":null}\n",
        );
        let loaded = cases(&path).expect("an explicit null in loads");
        let case = loaded.first().expect("the row is present");
        assert_eq!(case.input, Json::Null);
    }
}
