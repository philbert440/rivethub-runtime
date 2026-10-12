use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use config::{Finding, PLUGINS_IGNORED, ProviderType, WORKFLOWS_IGNORED};
use libtest_mimic::{Arguments, Failed, Trial};

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

#[derive(serde::Deserialize)]
struct Case {
    #[serde(rename = "in")]
    input: serde_yaml_ng::Value,
    out: Recorded,
}

#[derive(serde::Deserialize)]
struct Recorded {
    #[serde(default)]
    errors: Vec<Finding>,
    #[serde(default)]
    warnings: Vec<Finding>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct Deviation {
    id: String,
    class: DeviationClass,
    #[serde(default)]
    findings: Vec<Finding>,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, strum::Display,
)]
enum DeviationClass {
    #[serde(rename = "rejected")]
    #[strum(serialize = "accepted-but-rejected")]
    Rejected,
    #[serde(rename = "order")]
    #[strum(serialize = "order-only Available lists")]
    Order,
    #[serde(rename = "seq_form")]
    #[strum(serialize = "sequence-shaped sections or entries")]
    SeqForm,
    #[serde(rename = "null_missing")]
    #[strum(serialize = "explicit nulls read as missing")]
    NullMissing,
}

struct Replay {
    id: String,
    input: serde_yaml_ng::Value,
    recorded_accepted: bool,
    contract: Vec<Finding>,
    deviation: Option<Deviation>,
}

fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("fixtures")
        .join(format!("{name}.jsonl"))
}

fn golden_path(subpath: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join(subpath)
}

fn read_cases(name: &str) -> Result<Vec<Case>, String> {
    let path = fixture_path(name);
    let text = fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| format!("{}: {error}", path.display()))
        })
        .collect()
}

fn load_deviations() -> Result<Vec<Deviation>, String> {
    let path = golden_path("deviations.jsonl");
    let text = fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| format!("{}: {error}", path.display()))
        })
        .collect()
}

fn section_value<'a>(
    input: &'a serde_yaml_ng::Value,
    key: &str,
) -> Option<&'a serde_yaml_ng::Value> {
    if input.is_mapping() {
        input.get(key)
    } else {
        None
    }
}

fn removed_provider_ids(input: &serde_yaml_ng::Value) -> Vec<(String, ProviderType)> {
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
        let declared = entry.get("type").and_then(serde_yaml_ng::Value::as_str);
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

const DECISION_15_WARNING: &str = "memory.capture.redaction is validated but not yet injected into harness hook processes; set RIVETOS_CAPTURE_REDACTION or pass CaptureWriterOptions.redaction to enable at runtime";
const OPENAI_COMPAT_SPLIT: &str = "The \"openai-compat\" provider was split into dedicated providers. Use \"vllm\" for a vLLM server, or \"llama-server\" for llama.cpp's llama-server — both take base_url.";

fn decision_replacements(input: &serde_yaml_ng::Value) -> Vec<(String, bool, String, String)> {
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

fn contract_findings(input: &serde_yaml_ng::Value, recorded: &Recorded) -> Vec<Finding> {
    let replacements = decision_replacements(input);
    let mut findings: Vec<Finding> = recorded
        .errors
        .iter()
        .chain(&recorded.warnings)
        .filter(|finding| finding.message != DECISION_15_WARNING)
        .filter(|finding| {
            !replacements
                .iter()
                .any(|(scope, _, _, _)| covered(&finding.path, scope))
        })
        .cloned()
        .collect();
    for (_, error, path, message) in replacements {
        findings.push(Finding {
            severity: if error {
                config::Severity::Error
            } else {
                config::Severity::Warning
            },
            path,
            message,
        });
    }
    findings
}

fn multiset(findings: &[Finding]) -> BTreeSet<(String, String, String)> {
    findings
        .iter()
        .map(|finding| {
            (
                finding.severity.to_string(),
                finding.path.clone(),
                finding.message.clone(),
            )
        })
        .collect()
}

fn replay(case: &Replay) -> Result<(), Failed> {
    let actual = config::validate_document(&case.input);
    let actual_findings: Vec<Finding> = actual
        .errors
        .iter()
        .chain(&actual.warnings)
        .cloned()
        .collect();
    if config::type_document(&case.input).is_err() {
        if actual.errors.len() != 1 {
            return Err(format!(
                "{}: expected exactly one wrong-type error, got {}: {:?}",
                case.id,
                actual.errors.len(),
                multiset(&actual.errors)
            )
            .into());
        }
        return match case.deviation.as_ref().map(|deviation| deviation.class) {
            Some(DeviationClass::Rejected) if case.recorded_accepted => Ok(()),
            Some(DeviationClass::Rejected) => Err(format!(
                "{}: the deviations table pins accepted-but-rejected, but the recording rejects this input",
                case.id
            )
            .into()),
            Some(class) => Err(format!(
                "{}: the document is type-confused but the deviations table pins class {class:?}",
                case.id
            )
            .into()),
            None if case.recorded_accepted => Err(format!(
                "{}: accepted by the recording but rejected here, and absent from the deviations table: {:?}",
                case.id,
                multiset(&actual.errors)
            )
            .into()),
            None => Ok(()),
        };
    }
    let expected = match &case.deviation {
        Some(Deviation {
            class: DeviationClass::Rejected,
            ..
        }) => {
            return Err(format!(
                "{}: the deviations table pins accepted-but-rejected, but the document is well-typed",
                case.id
            )
            .into());
        }
        Some(deviation) => &deviation.findings,
        None => &case.contract,
    };
    let (expected_set, actual_set) = (multiset(expected), multiset(&actual_findings));
    if expected_set == actual_set {
        return Ok(());
    }
    let expected_only: Vec<_> = expected_set.difference(&actual_set).collect();
    let actual_only: Vec<_> = actual_set.difference(&expected_set).collect();
    Err(format!(
        "{}: findings diverge\n  expected-only: {expected_only:?}\n  actual-only:   {actual_only:?}",
        case.id
    )
    .into())
}

fn deviations_are_documented() -> Result<(), Failed> {
    let text = fs::read_to_string(golden_path("DEVIATIONS.md"))
        .map_err(|error| format!("DEVIATIONS.md: {error}"))?;
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
        if !text.contains(marker) {
            return Err(format!("DEVIATIONS.md must document {marker}").into());
        }
    }
    for file in GOLDEN_FILES {
        if !text.contains(&format!("{file}.jsonl")) {
            return Err(format!("DEVIATIONS.md must name {file}.jsonl").into());
        }
    }
    let rows = load_deviations().map_err(Failed::from)?;
    let mut counts: BTreeMap<(String, DeviationClass), usize> = BTreeMap::new();
    for row in &rows {
        let Some((file, _)) = row.id.split_once(':') else {
            return Err(format!("deviations row without a line number: {}", row.id).into());
        };
        *counts.entry((file.to_owned(), row.class)).or_default() += 1;
    }
    for ((file, class), count) in counts {
        let bullet = format!("- `{file}` — {count} {class}");
        if !text.contains(&bullet) {
            return Err(format!("DEVIATIONS.md must contain: {bullet}").into());
        }
    }
    Ok(())
}

fn collect_trials() -> Result<Vec<Trial>, String> {
    let deviations: BTreeMap<String, Deviation> = load_deviations()?
        .into_iter()
        .map(|row| (row.id.clone(), row))
        .collect();
    let mut trials = Vec::new();
    for name in GOLDEN_FILES {
        for (index, case) in read_cases(name)?.into_iter().enumerate() {
            let id = format!("{name}.jsonl:{}", index + 1);
            let recorded_accepted = case.out.errors.is_empty();
            let contract = contract_findings(&case.input, &case.out);
            let deviation = deviations.get(&id).cloned();
            let input = case.input;
            trials.push(Trial::test(id.clone(), move || {
                replay(&Replay {
                    id,
                    input,
                    recorded_accepted,
                    contract,
                    deviation,
                })
            }));
        }
    }
    trials.push(Trial::test(
        "deviations_are_documented".to_owned(),
        deviations_are_documented,
    ));
    Ok(trials)
}

fn main() {
    let arguments = Arguments::from_args();
    let trials = match collect_trials() {
        Ok(trials) => trials,
        Err(error) => vec![Trial::test("collect_cases", move || Err(error.into()))],
    };
    libtest_mimic::run(&arguments, trials).exit();
}
