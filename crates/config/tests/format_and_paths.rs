use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Deserialize)]
struct FormatCase {
    #[serde(rename = "in")]
    input: FormatInput,
    out: String,
}

#[derive(Deserialize)]
struct FormatInput {
    #[serde(default)]
    errors: Vec<config::Finding>,
    #[serde(default)]
    warnings: Vec<config::Finding>,
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("fixtures")
        .join(format!("{name}.jsonl"))
}

fn read_lines(name: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(fixture(name))?;
    Ok(text
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

fn parse_case<T: serde::de::DeserializeOwned>(
    name: &str,
    index: usize,
    line: &str,
) -> Result<T, Box<dyn std::error::Error>> {
    let label = format!("{name} case {}", index + 1);
    serde_json::from_str(line).map_err(|error| format!("parse {label}: {error}").into())
}

#[test]
fn format_validation_result_replay() -> Result<(), Box<dyn std::error::Error>> {
    let mut failures = Vec::new();
    for (index, line) in read_lines("format_validation_result")?.iter().enumerate() {
        let case: FormatCase = parse_case("format_validation_result", index, line)?;
        let mut result = config::ValidationResult::default();
        for finding in case.input.errors {
            result.push(finding);
        }
        for finding in case.input.warnings {
            result.push(finding);
        }
        let actual = result.to_string();
        if actual != case.out && failures.len() < 5 {
            failures.push(format!(
                "case {}\n  expected: {:?}\n  actual:   {:?}",
                index + 1,
                case.out,
                actual
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} format cases diverge:\n{}",
        failures.len(),
        failures.join("\n")
    );
    Ok(())
}

#[derive(Deserialize)]
struct PathsCase {
    #[serde(rename = "in")]
    input: PathsInput,
    out: String,
}

#[derive(Deserialize)]
struct PathsInput {
    env: BTreeMap<String, String>,
    #[serde(default)]
    segments: Option<Vec<String>>,
}

fn replay_paths(
    name: &str,
    apply: fn(&config::Environment, Option<Vec<String>>) -> PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut failures = Vec::new();
    for (index, line) in read_lines(name)?.iter().enumerate() {
        let case: PathsCase = parse_case(name, index, line)?;
        let env = config::Environment::from_vars(case.input.env.clone());
        let actual = apply(&env, case.input.segments.clone());
        if actual != Path::new(case.out.as_str()) && failures.len() < 5 {
            failures.push(format!(
                "{name} case {}\n  env: {:?}\n  expected: {}\n  actual:   {}",
                index + 1,
                case.input.env,
                case.out,
                actual.display()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} {name} cases diverge:\n{}",
        failures.len(),
        failures.join("\n")
    );
    Ok(())
}

fn shared_dir(env: &config::Environment, _segments: Option<Vec<String>>) -> PathBuf {
    config::shared_dir(env)
}

fn install_root(env: &config::Environment, _segments: Option<Vec<String>>) -> PathBuf {
    config::install_root(env)
}

fn shared_path(env: &config::Environment, segments: Option<Vec<String>>) -> PathBuf {
    let owned: Vec<String> = segments.unwrap_or_default();
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    config::shared_path(env, &refs)
}

fn install_path(env: &config::Environment, segments: Option<Vec<String>>) -> PathBuf {
    let owned: Vec<String> = segments.unwrap_or_default();
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    config::install_path(env, &refs)
}

#[test]
fn shared_dir_replay() -> Result<(), Box<dyn std::error::Error>> {
    replay_paths("shared_dir", shared_dir)
}

#[test]
fn install_root_replay() -> Result<(), Box<dyn std::error::Error>> {
    replay_paths("install_root", install_root)
}

#[test]
fn shared_path_replay() -> Result<(), Box<dyn std::error::Error>> {
    replay_paths("shared_path", shared_path)
}

#[test]
fn install_path_replay() -> Result<(), Box<dyn std::error::Error>> {
    replay_paths("install_path", install_path)
}
