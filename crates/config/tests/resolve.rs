use std::collections::BTreeMap;

use serde::Deserialize;

#[expect(
    clippy::redundant_pub_crate,
    reason = "private shared test module: unreachable_pub requires pub(crate) here, the nursery lint disagrees"
)]
mod common;

#[derive(Deserialize)]
struct ResolveCase {
    #[serde(rename = "in")]
    input: ResolveInput,
    out: serde_yaml_ng::Value,
}

#[derive(Deserialize, serde::Serialize)]
struct ResolveInput {
    env: BTreeMap<String, String>,
    value: serde_yaml_ng::Value,
}

#[test]
fn resolve_env_vars_replay() -> Result<(), Box<dyn std::error::Error>> {
    let lines = common::read_cases("resolve_env_vars")?;
    let mut failures = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let case: ResolveCase = common::parse_case("resolve_env_vars", index, line)?;
        let env = config::Environment::from_vars(case.input.env.clone());
        let mut actual = case.input.value.clone();
        config::resolve_env_vars_with(&env, &mut actual);
        if actual != case.out {
            failures.push(common::case_failure(
                &common::case_id("resolve_env_vars", index),
                "resolved value diverges",
                &case.input,
                &common::json_text(&case.out),
                &common::json_text(&actual),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{}/{} resolve_env_vars.jsonl cases diverge:\n\n{}",
        failures.len(),
        lines.len(),
        failures.join("\n\n")
    );
    Ok(())
}
