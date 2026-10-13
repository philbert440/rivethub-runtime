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

pub fn cases(path: &Path) -> Result<Vec<DiffCase>, Error> {
    let text = crate::read_text(path)?;
    let mut cases = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        if line.trim().is_empty() {
            continue;
        }
        let record: Json = serde_json::from_str(line)
            .map_err(|error| Error::format(path, format!("{number}: {error}")))?;
        let Some(function) = record.get("fn").and_then(Json::as_str) else {
            return Err(Error::format(path, format!("{number}: case has no \"fn\"")));
        };
        let function = function.to_owned();
        let Some(input) = record.get("in").cloned() else {
            return Err(Error::format(path, format!("{number}: case has no \"in\"")));
        };
        let out_present = record.get("out").is_some();
        let throws = record
            .get("throws")
            .and_then(|thrown| thrown.get("name"))
            .and_then(Json::as_str)
            .map(str::to_owned);
        let expected = match (out_present, throws) {
            (true, None) => Expected::Out(record.get("out").cloned().unwrap_or(Json::Null)),
            (false, Some(name)) => Expected::Throws(name),
            _ => {
                return Err(Error::format(
                    path,
                    format!("{number}: case must have exactly one of out and throws"),
                ));
            }
        };
        cases.push(DiffCase {
            line: number,
            function,
            input,
            expected,
        });
    }
    Ok(cases)
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

    fn write_row(name: &str, row: &str) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("rr-case-{name}-{}.jsonl", std::process::id()));
        std::fs::write(&path, row).expect("the temp row writes");
        path
    }

    #[test]
    fn a_row_without_in_is_a_load_error() {
        let path = write_row(
            "missing-in",
            "{\"fn\":\"normalize_api_prefix\",\"out\":null}\n",
        );
        let error = cases(&path).expect_err("missing in fails the load");
        let text = error.to_string();
        assert!(text.contains("case has no \"in\""), "{text}");
        std::fs::remove_file(&path).expect("the temp row removes");
    }

    #[test]
    fn an_explicit_null_in_still_loads() {
        let path = write_row(
            "null-in",
            "{\"fn\":\"normalize_api_prefix\",\"in\":null,\"out\":null}\n",
        );
        let loaded = cases(&path).expect("an explicit null in loads");
        let case = loaded.first().expect("the row is present");
        assert_eq!(case.input, Json::Null);
        std::fs::remove_file(&path).expect("the temp row removes");
    }
}
