use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Deserialize)]
struct ResolveCase {
    #[serde(rename = "in")]
    input: ResolveInput,
    out: serde_yaml_ng::Value,
}

#[derive(Deserialize)]
struct ResolveInput {
    env: BTreeMap<String, String>,
    value: serde_yaml_ng::Value,
}

fn golden_path(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("fixtures")
        .join(format!("{name}.jsonl"))
}

#[test]
fn resolve_env_vars_replay() {
    let text = std::fs::read_to_string(golden_path("resolve_env_vars")).unwrap();
    let mut failures = Vec::new();
    for (index, line) in text.lines().filter(|line| !line.is_empty()).enumerate() {
        let case: ResolveCase = serde_json::from_str(line)
            .unwrap_or_else(|error| panic!("parse resolve case {}: {error}", index + 1));
        let env = config::Environment::from_vars(case.input.env.clone());
        let mut actual = case.input.value.clone();
        config::resolve_env_vars_with(&env, &mut actual);
        if actual != case.out && failures.len() < 5 {
            failures.push(format!(
                "case {}\n  input: {:?}\n  expected: {:?}\n  actual:   {:?}",
                index + 1,
                case.input.value,
                case.out,
                actual
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} resolve cases diverge:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
