use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use regex::Captures;
use regex::Regex;
use serde::Deserialize;

use crate::Error;
use crate::Mismatch;
use crate::capture::ISO_TIMESTAMP;
use crate::json::Json;

#[derive(Debug, Clone)]
pub struct PgStep {
    pub kind: String,
    pub label: String,
    pub input: Json,
}

#[derive(Debug, Default)]
pub struct PgOutcome {
    pub result: Option<Json>,
    pub error: Option<String>,
    pub rows: Option<Json>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Recording {
    #[serde(default)]
    pub scenarios: Vec<ScenarioRecord>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ScenarioRecord {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub steps: Vec<StepRecord>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct StepRecord {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub input: Json,
    #[serde(default)]
    pub result: Option<Json>,
    #[serde(default)]
    pub error: Option<ErrorRecord>,
    #[serde(default)]
    pub rows: Option<Json>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ErrorRecord {
    #[serde(default)]
    pub name: String,
}

static ISO_ONLY: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(ISO_TIMESTAMP).ok());

static UUID: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"\A[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\z")
        .ok()
});

fn looks_like_uuid(text: &str) -> bool {
    UUID.as_ref().is_some_and(|uuid| uuid.is_match(text))
}

fn placeholder_category(text: &str) -> Option<(&'static str, String)> {
    for (prefix, category) in [
        ("conv-", "conv"),
        ("msg-", "msg"),
        ("id-", "id"),
        ("<now-", "now"),
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            let digits = rest.trim_end_matches('>');
            if !digits.is_empty() && digits.bytes().all(|c| c.is_ascii_digit()) {
                return Some((category, digits.to_string()));
            }
        }
    }
    None
}

fn rewrite_fixtures(text: &str, root: Option<&str>) -> String {
    let Some(root) = root else {
        return text.to_string();
    };
    let rel = root.strip_prefix('/').unwrap_or(root);
    text.replace(root, "__FIXTURES__")
        .replace(rel, "__FIXTURES_REL__")
}

#[derive(Debug, Default)]
struct PgNorm {
    categories: BTreeMap<String, String>,
    assigned: BTreeMap<String, String>,
    counters: BTreeMap<String, usize>,
}

impl PgNorm {
    fn placeholder(&mut self, text: &str, category: &str) -> String {
        if let Some(seen) = self.assigned.get(text) {
            return seen.clone();
        }
        let counter = self.counters.entry(category.to_string()).or_default();
        *counter += 1;
        let placeholder = if category == "now" {
            format!("<now-{counter}>")
        } else {
            format!("{category}-{counter}")
        };
        self.assigned.insert(text.to_string(), placeholder.clone());
        placeholder
    }

    fn collect_categories(&mut self, rows: &Json) {
        let Some(tables) = rows.as_object() else {
            return;
        };
        for (table, dumps) in tables {
            let conversations = table == "ros_conversations";
            let messages = table == "ros_messages";
            if !conversations && !messages {
                continue;
            }
            let category = if conversations { "conv" } else { "msg" };
            for row in dumps.as_array().unwrap_or(&Vec::new()) {
                if let Some(id) = row.get("id").and_then(Json::as_str)
                    && looks_like_uuid(id)
                {
                    self.categories.insert(id.to_string(), category.to_string());
                }
            }
        }
    }

    fn normalize_text(&mut self, text: &str, kept: &[String]) -> String {
        let Some(iso) = ISO_ONLY.as_ref() else {
            return text.to_string();
        };
        let replaced = iso.replace_all(text, |caps: &Captures<'_>| {
            let matched = caps.get(0).map_or("", |found| found.as_str());
            self.placeholder(matched, "now")
        });
        let out = replaced.into_owned();
        if looks_like_uuid(&out) && !kept.iter().any(|k| k.eq_ignore_ascii_case(&out)) {
            let category = self
                .categories
                .get(&out)
                .cloned()
                .unwrap_or_else(|| "id".to_string());
            return self.placeholder(&out, &category);
        }
        out
    }

    fn normalize_value(
        &mut self,
        value: &mut Json,
        table: Option<&str>,
        column: Option<&str>,
        kept: &[String],
        root: Option<&str>,
    ) {
        match value {
            Json::Object(map) => {
                for (name, member) in map.iter_mut() {
                    let member_column = if table.is_some() {
                        Some(name.as_str())
                    } else {
                        column
                    };
                    self.normalize_value(member, table.or(Some(name)), member_column, kept, root);
                }
            }
            Json::Array(items) => {
                for item in items {
                    self.normalize_value(item, table, column, kept, root);
                }
            }
            Json::String(text) => {
                *text = rewrite_fixtures(text, root);
                *text = self.normalize_text(text, kept);
            }
            Json::Null | Json::Bool(_) | Json::Number(_) => {}
        }
    }
}

fn collect_input_uuids(value: &Json, out: &mut Vec<String>) {
    match value {
        Json::Object(map) => {
            for member in map.values() {
                collect_input_uuids(member, out);
            }
        }
        Json::Array(items) => {
            for item in items {
                collect_input_uuids(item, out);
            }
        }
        Json::String(text) => {
            if looks_like_uuid(text) && !out.iter().any(|k| k.eq_ignore_ascii_case(text)) {
                out.push(text.clone());
            }
        }
        Json::Null | Json::Bool(_) | Json::Number(_) => {}
    }
}

fn pg_value_eq(rec: &Json, act: &Json, map: &mut BTreeMap<(String, String), String>) -> bool {
    match (rec, act) {
        (Json::Object(r), Json::Object(a)) => {
            r.len() == a.len()
                && r.iter()
                    .all(|(k, rv)| a.get(k).is_some_and(|av| pg_value_eq(rv, av, map)))
        }
        (Json::Array(r), Json::Array(a)) => {
            r.len() == a.len() && r.iter().zip(a).all(|(x, y)| pg_value_eq(x, y, map))
        }
        (Json::String(r), Json::String(a)) => {
            match (placeholder_category(r), placeholder_category(a)) {
                (Some((rc, rn)), Some((ac, an))) => {
                    if rc != ac {
                        return false;
                    }
                    let key = (rc.to_string(), rn);
                    if let Some(seen) = map.get(&key) {
                        seen == &an
                    } else {
                        map.insert(key, an);
                        true
                    }
                }
                (None, None) => r == a,
                _ => false,
            }
        }
        (Json::Number(r), Json::Number(a)) => r == a || r.as_f64() == a.as_f64(),
        (Json::Null, Json::Null) => true,
        (Json::Bool(r), Json::Bool(a)) => r == a,
        _ => false,
    }
}

fn compare_pg_step(
    id: &str,
    step: &StepRecord,
    actual: &PgOutcome,
    map: &mut BTreeMap<(String, String), String>,
) -> Result<(), Mismatch> {
    if step.result.is_none() && step.error.is_none() && step.rows.is_none() {
        return Err(Mismatch::new(
            id,
            "recording has neither result, error nor rows",
        ));
    }
    match (&actual.result, &actual.error) {
        (Some(_), Some(_)) => {
            return Err(Mismatch::new(
                id,
                "step produced both a result and an error",
            ));
        }
        (Some(actual_result), _) => {
            let Some(rec_result) = &step.result else {
                return Err(Mismatch::new(
                    id,
                    "step returned a result; the recording recorded none",
                ));
            };
            if !pg_value_eq(rec_result, actual_result, map) {
                return Err(Mismatch::new(
                    id,
                    format!(
                        "result mismatch\n    recorded: {rec_result}\n    actual:   {actual_result}"
                    ),
                ));
            }
        }
        (None, Some(actual_error)) => {
            let Some(rec_error) = &step.error else {
                return Err(Mismatch::new(
                    id,
                    "step errored; the recording recorded no error",
                ));
            };
            if rec_error.name != *actual_error {
                return Err(Mismatch::new(
                    id,
                    format!("error {actual_error:?} != recorded {:?}", rec_error.name),
                ));
            }
        }
        (None, None) => {}
    }
    match (&step.rows, &actual.rows) {
        (None, None) => {}
        (None, Some(_)) => {
            return Err(Mismatch::new(id, "unexpected rows"));
        }
        (Some(want_rows), Some(actual_rows)) => {
            if !pg_value_eq(want_rows, actual_rows, map) {
                return Err(Mismatch::new(
                    id,
                    format!(
                        "rows mismatch\n    recorded: {want_rows}\n    actual:   {actual_rows}"
                    ),
                ));
            }
        }
        (Some(_), None) => {
            return Err(Mismatch::new(id, "recorded rows missing"));
        }
    }
    Ok(())
}

pub fn scenarios(recording: &Path) -> Result<Vec<ScenarioRecord>, Error> {
    let raw = crate::read_text(recording)?;
    let sanitized = crate::capture::sanitize_surrogates(&raw);
    let doc: Recording = crate::parse_json(recording, &sanitized)?;
    Ok(doc.scenarios)
}

pub fn replay_scenario(
    scenario: &ScenarioRecord,
    fixtures_root: Option<&str>,
    mut run_step: impl FnMut(&PgStep) -> PgOutcome,
) -> Result<(), Mismatch> {
    let mut kept = Vec::new();
    for step in &scenario.steps {
        collect_input_uuids(&step.input, &mut kept);
    }
    let mut norm = PgNorm::default();
    let mut map = BTreeMap::new();
    for step in &scenario.steps {
        let label = if step.label.is_empty() {
            step.kind.clone()
        } else {
            step.label.clone()
        };
        let id = format!("{}/{}", scenario.name, label);
        let call = PgStep {
            kind: step.kind.clone(),
            label: step.label.clone(),
            input: step.input.clone(),
        };
        let mut actual = run_step(&call);
        if let Some(rows) = actual.rows.as_ref() {
            norm.collect_categories(rows);
        }
        if let Some(result) = actual.result.as_mut() {
            norm.normalize_value(result, None, None, &kept, fixtures_root);
        }
        if let Some(rows) = actual.rows.as_mut() {
            norm.normalize_value(rows, None, None, &kept, fixtures_root);
        }
        compare_pg_step(&id, step, &actual, &mut map)?;
    }
    Ok(())
}
