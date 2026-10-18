use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use wire::task::{AcceptanceCriterion, ContextRef, EvalOutcome, TaskBudget, TaskResult, TaskUsage};
use wire::task_overrides::TaskOverrides;

#[derive(Deserialize)]
struct Fixture {
    budget: Vec<Case>,
    usage: Vec<Case>,
    result: Vec<Case>,
    spec: Vec<Case>,
    eval: Vec<Case>,
    context_refs: Vec<Case>,
    acceptance_criteria: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    input: Value,
    expected: Value,
}

#[expect(
    clippy::expect_used,
    reason = "invalid fixture data must fail the replay"
)]
fn replay<T: DeserializeOwned + Serialize>(cases: &[Case]) {
    for case in cases {
        let typed: T = serde_json::from_value(case.input.clone()).expect("typed input");
        assert_eq!(
            serde_json::to_value(typed).expect("serialized"),
            case.expected
        );
    }
}

#[test]
fn task_json_columns_omit_absent_members_and_keep_present_limits() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/tasks/json-columns.json")).expect("fixture");
    replay::<TaskBudget>(&fixture.budget);
    replay::<TaskUsage>(&fixture.usage);
    replay::<TaskResult>(&fixture.result);
    replay::<TaskOverrides>(&fixture.spec);
    replay::<EvalOutcome>(&fixture.eval);
    replay::<Vec<ContextRef>>(&fixture.context_refs);
    replay::<Vec<AcceptanceCriterion>>(&fixture.acceptance_criteria);
}

#[test]
fn invalid_limits_and_delegation_metadata_are_typed_errors() {
    for text in [
        r#"{"maxTurns":"1"}"#,
        r#"{"maxTokens":-1}"#,
        r#"{"maxWallClockMs":1.5}"#,
    ] {
        assert!(serde_json::from_str::<TaskBudget>(text).is_err());
    }
    for text in [r#"{"delegation":"true"}"#, r#"{"meshFrom":42}"#] {
        assert!(serde_json::from_str::<TaskOverrides>(text).is_err());
    }
}
