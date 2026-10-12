use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use config::{Finding, PLUGINS_IGNORED, ProviderType, ValidationResult, WORKFLOWS_IGNORED};

#[derive(serde::Deserialize)]
struct Case {
    #[serde(rename = "in")]
    input: serde_yaml::Value,
    out: Expectation,
}

#[derive(serde::Deserialize)]
struct Expectation {
    #[serde(default)]
    errors: Vec<Finding>,
    #[serde(default)]
    warnings: Vec<Finding>,
}

const DECISION_15_WARNING: &str = "memory.capture.redaction is validated but not yet injected into harness hook processes; set RIVETOS_CAPTURE_REDACTION or pass CaptureWriterOptions.redaction to enable at runtime";
const OPENAI_COMPAT_SPLIT: &str = "The \"openai-compat\" provider was split into dedicated providers. Use \"vllm\" for a vLLM server, or \"llama-server\" for llama.cpp's llama-server — both take base_url.";

const GOLDEN_FILES: [&str; 8] = [
    "validate_config",
    "validate_config_runtime",
    "validate_config_agents_providers",
    "validate_config_memory",
    "validate_config_den",
    "validate_config_tasks_workflows",
    "validate_config_mesh_channels_misc",
    "validate_config_untyped",
];

fn golden_path(name: &str) -> String {
    format!(
        "{}/tests/golden/fixtures/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn read_cases(name: &str) -> Result<Vec<Case>, Box<dyn std::error::Error>> {
    let text = fs::read_to_string(golden_path(name))?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_yaml::from_str(line).map_err(std::convert::Into::into))
        .collect()
}

fn section_value<'a>(input: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    if input.is_mapping() {
        input.get(key)
    } else {
        None
    }
}

fn removed_provider_ids(input: &serde_yaml::Value) -> Vec<(String, ProviderType)> {
    let Some(providers) =
        section_value(input, "providers").and_then(|providers| providers.as_mapping())
    else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for (id, entry) in providers {
        let Some(id) = id.as_str() else {
            continue;
        };
        let declared = entry.get("type").and_then(serde_yaml::Value::as_str);
        let kind = match declared {
            Some(text) => text.parse::<ProviderType>().ok(),
            None => id.parse::<ProviderType>().ok(),
        };
        match kind {
            Some(kind) if kind.is_removed() || kind == ProviderType::OpenaiCompat => {
                removed.push((id.to_owned(), kind));
            }
            Some(_) | None => {}
        }
    }
    removed
}

fn decision_replacements(input: &serde_yaml::Value) -> Vec<(String, bool, String, String)> {
    let mut replacements = Vec::new();
    if section_value(input, "plugins").is_some() {
        replacements.push((
            "plugins".to_owned(),
            false,
            "plugins".to_owned(),
            PLUGINS_IGNORED.to_owned(),
        ));
    }
    if section_value(input, "workflows").is_some() {
        replacements.push((
            "workflows".to_owned(),
            false,
            "workflows".to_owned(),
            WORKFLOWS_IGNORED.to_owned(),
        ));
    }
    for (id, kind) in removed_provider_ids(input) {
        let (path, message) = match kind.removed_harness() {
            Some(harness) => (
                format!("providers.{id}.type"),
                format!(
                    "Provider \"{id}\" type \"{kind}\" is not built — reach {kind} models through the {harness} harness instead."
                ),
            ),
            None => (format!("providers.{id}"), OPENAI_COMPAT_SPLIT.to_owned()),
        };
        replacements.push((format!("providers.{id}"), true, path, message));
    }
    replacements
}

fn covered(path: &str, section: &str) -> bool {
    path == section
        || path.starts_with(&format!("{section}."))
        || path.starts_with(&format!("{section}["))
}

fn expected_outcome(input: &serde_yaml::Value, expectation: &Expectation) -> ValidationResult {
    let mut result = ValidationResult::default();
    let replaced = decision_replacements(input);
    let is_replaced = |finding: &Finding| {
        replaced
            .iter()
            .any(|(scope, _, _, _)| covered(&finding.path, scope))
    };
    for finding in expectation
        .errors
        .iter()
        .chain(&expectation.warnings)
        .filter(|finding| finding.message != DECISION_15_WARNING)
        .filter(|finding| !is_replaced(finding))
    {
        result.push(finding.clone());
    }
    for (_, error, path, message) in replaced {
        if error {
            result.error(path, message);
        } else {
            result.warning(path, message);
        }
    }
    result
}

fn canonical(message: &str) -> String {
    let Some((prefix, names)) = message.split_once("Available: ") else {
        return message.to_owned();
    };
    let mut names: Vec<&str> = names.split(", ").collect();
    names.sort_unstable();
    format!("{prefix}Available: {}", names.join(", "))
}

fn canonical_multiset(result: &ValidationResult) -> BTreeSet<(String, String, String)> {
    result
        .errors
        .iter()
        .chain(&result.warnings)
        .map(|finding| {
            (
                finding.severity.to_string(),
                finding.path.clone(),
                canonical(&finding.message),
            )
        })
        .collect()
}

fn raw_multiset(result: &ValidationResult) -> BTreeSet<(String, String, String)> {
    result
        .errors
        .iter()
        .chain(&result.warnings)
        .map(|finding| {
            (
                finding.severity.to_string(),
                finding.path.clone(),
                finding.message.clone(),
            )
        })
        .collect()
}

#[derive(Default)]
struct Counters {
    rejected: usize,
    rejected_examples: Vec<String>,
    order_only: usize,
    order_only_examples: Vec<String>,
    seq_form: usize,
    seq_form_examples: Vec<String>,
    null_missing: usize,
    null_missing_examples: Vec<String>,
}

fn is_object_shape(message: &str) -> bool {
    message.ends_with("must be an object")
        || message.ends_with("must be an object mapping agent names to their config")
        || message.ends_with("must be an object mapping provider names to their config")
        || message.ends_with("must be an object mapping transport names to their config")
        || message.ends_with("must be an object (e.g. { enabled: true })")
        || message.ends_with("Each heartbeat entry must be an object")
}

fn seq_form_divergence(
    expected_only: &[&(String, String, String)],
    actual_only: &[&(String, String, String)],
) -> bool {
    for (_, path, message) in expected_only {
        if !is_object_shape(message) {
            continue;
        }
        let contained = actual_only
            .iter()
            .all(|(_, other, _)| other == path || other.starts_with(&format!("{path}.")));
        if contained {
            return true;
        }
    }
    false
}

fn value_at_path<'a>(input: &'a serde_yaml::Value, path: &str) -> Option<&'a serde_yaml::Value> {
    let mut current = input;
    for segment in path.split('.') {
        let (name, index) = match segment
            .strip_suffix(']')
            .and_then(|rest| rest.rsplit_once('['))
        {
            Some((name, index)) => (name, Some(index)),
            None => (segment, None),
        };
        current = current.get(name)?;
        if let Some(index) = index {
            current = current.get(index.parse::<usize>().ok()?)?;
        }
    }
    Some(current)
}

fn null_missing_divergence(
    input: &serde_yaml::Value,
    expected_only: &[&(String, String, String)],
    actual_only: &[&(String, String, String)],
) -> bool {
    actual_only.is_empty()
        && !expected_only.is_empty()
        && expected_only.iter().all(|(severity, path, _)| {
            severity == "error"
                && value_at_path(input, path).is_some_and(serde_yaml::Value::is_null)
        })
}

fn compare(name: &str, cases: &[Case], failures: &mut Vec<String>) -> Counters {
    let mut counters = Counters::default();
    for (index, case) in cases.iter().enumerate() {
        let id = format!("{name}.jsonl:{}", index + 1);
        let actual = config::validate_document(&case.input);
        let expected = expected_outcome(&case.input, &case.out);
        if config::type_document(&case.input).is_ok() {
            if canonical_multiset(&expected) == canonical_multiset(&actual) {
                if raw_multiset(&expected) != raw_multiset(&actual) {
                    counters.order_only += 1;
                    if counters.order_only_examples.len() < 5 {
                        counters.order_only_examples.push(id);
                    }
                }
            } else {
                let expected_set = raw_multiset(&expected);
                let actual_set = raw_multiset(&actual);
                let expected_only: Vec<_> = expected_set.difference(&actual_set).collect();
                let actual_only: Vec<_> = actual_set.difference(&expected_set).collect();
                if null_missing_divergence(&case.input, &expected_only, &actual_only) {
                    counters.null_missing += 1;
                    if counters.null_missing_examples.len() < 5 {
                        counters.null_missing_examples.push(id);
                    }
                } else if seq_form_divergence(&expected_only, &actual_only) {
                    counters.seq_form += 1;
                    if counters.seq_form_examples.len() < 5 {
                        counters.seq_form_examples.push(id);
                    }
                } else if failures.len() < 60 {
                    failures.push(format!(
                        "{id}\n  expected-only: {expected_only:?}\n  actual-only:   {actual_only:?}"
                    ));
                }
            }
        } else {
            if actual.errors.len() != 1 {
                failures.push(format!(
                    "{id}\n  expected exactly one wrong-type error, got: {:?}",
                    raw_multiset(&actual)
                ));
                continue;
            }
            if case.out.errors.is_empty() {
                counters.rejected += 1;
                if counters.rejected_examples.len() < 5 {
                    counters.rejected_examples.push(id);
                }
            }
        }
    }
    counters
}

fn counters_text(counters: &BTreeMap<&'static str, Counters>) -> String {
    let mut text = String::new();
    for (name, counter) in counters {
        if counter.rejected > 0 {
            text.push_str(&counters_line(
                name,
                counter.rejected,
                "accepted-but-rejected",
                &counter.rejected_examples,
            ));
        }
        if counter.order_only > 0 {
            text.push_str(&counters_line(
                name,
                counter.order_only,
                "order-only Available lists",
                &counter.order_only_examples,
            ));
        }
        if counter.seq_form > 0 {
            text.push_str(&counters_line(
                name,
                counter.seq_form,
                "sequence-shaped sections or entries",
                &counter.seq_form_examples,
            ));
        }
        if counter.null_missing > 0 {
            text.push_str(&counters_line(
                name,
                counter.null_missing,
                "explicit nulls read as missing",
                &counter.null_missing_examples,
            ));
        }
    }
    text
}

fn counters_line(name: &str, count: usize, label: &str, examples: &[String]) -> String {
    format!(
        "- `{name}.jsonl` — {count} {label}; ids: {}\n",
        examples.join(", ")
    )
}

#[test]
fn validate_config_replays() -> Result<(), Box<dyn std::error::Error>> {
    let mut counters = BTreeMap::new();
    let mut failures = Vec::new();
    for name in GOLDEN_FILES {
        counters.insert(name, compare(name, &read_cases(name)?, &mut failures));
    }
    assert!(
        failures.is_empty(),
        "{} cases diverge:\n{}",
        failures.len(),
        failures.join("\n")
    );
    let documented = fs::read_to_string(format!(
        "{}/tests/golden/DEVIATIONS.md",
        env!("CARGO_MANIFEST_DIR")
    ))?;
    let mut missing = Vec::new();
    for line in counters_text(&counters).lines() {
        if !documented.contains(line.trim_end()) {
            missing.push(line.to_owned());
        }
    }
    assert!(
        missing.is_empty(),
        "DEVIATIONS.md must document the counted deviations:\n{}\ncounters:\n{}",
        missing.join("\n"),
        counters_text(&counters)
    );
    Ok(())
}

#[test]
fn deviations_are_documented() -> Result<(), Box<dyn std::error::Error>> {
    let text = fs::read_to_string(format!(
        "{}/tests/golden/DEVIATIONS.md",
        env!("CARGO_MANIFEST_DIR")
    ))?;
    for marker in [
        "Decision 15",
        "Decision 13",
        "Decision 12/20",
        "Decision 11",
        "Decision 9",
        "Decision 2",
        "Decision 16",
        "Decision 10",
    ] {
        assert!(
            text.contains(marker),
            "DEVIATIONS.md must document {marker}"
        );
    }
    for file in GOLDEN_FILES {
        assert!(
            text.contains(&format!("{file}.jsonl")),
            "DEVIATIONS.md must name {file}.jsonl"
        );
    }
    Ok(())
}
