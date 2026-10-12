use std::path::Path;

use serde::Deserialize;

use crate::Error;
use crate::Mismatch;
use crate::json::Json;
use crate::json::json_eq;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thrown(pub String);

#[derive(Debug, Clone, Default, Deserialize)]
struct CaseRecord {
    #[serde(rename = "fn", default)]
    function: String,
    #[serde(rename = "in", default)]
    input: Json,
    #[serde(default)]
    out: Option<Json>,
    #[serde(default)]
    throws: Option<ThrownRecord>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ThrownRecord {
    #[serde(default)]
    name: String,
}

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
        let record: CaseRecord = serde_json::from_str(line)
            .map_err(|error| Error::format(path, format!("{number}: {error}")))?;
        let expected = match (record.out, record.throws) {
            (Some(value), None) => Expected::Out(value),
            (None, Some(throws)) => Expected::Throws(throws.name),
            _ => {
                return Err(Error::format(
                    path,
                    format!("{number}: case must have exactly one of out and throws"),
                ));
            }
        };
        cases.push(DiffCase {
            line: number,
            function: record.function,
            input: record.input,
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
