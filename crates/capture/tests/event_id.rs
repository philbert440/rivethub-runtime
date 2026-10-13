use capture::event_id::OccurrenceKind;
use capture::json;

const TOOL_DIGEST: &str = "146febb19d952f1e7867c264405749f516bbd1a018c358ce43f86fa330080495";
const ASSISTANT_DIGEST: &str = "82c65e8f44c13f18ad14a0fb2031efafe4512258601b707cdb65d2d6c594fa54";

fn arguments(text: &str) -> Result<json::JsJson, capture::json::JsonError> {
    json::parse(text)
}

#[test]
fn tool_occurrence_digest_matches_the_recorded_id() -> Result<(), capture::json::JsonError> {
    let input = arguments(r#"{"file_path":"/work/demo/a.txt"}"#)?;
    assert_eq!(
        capture::event_id::occurrence_digest(
            OccurrenceKind::Tool,
            "[tool call] Read",
            "Read",
            Some(&input)
        ),
        TOOL_DIGEST
    );
    Ok(())
}

#[test]
fn assistant_occurrence_digest_matches_the_recorded_id() {
    assert_eq!(
        capture::event_id::occurrence_digest(OccurrenceKind::Assistant, "no uuid", "", None),
        ASSISTANT_DIGEST
    );
}

#[test]
fn occurrence_ids_carry_the_occurrence_index() -> Result<(), capture::json::JsonError> {
    let input = arguments(r#"{"file_path":"/work/demo/a.txt"}"#)?;
    let digest = capture::event_id::occurrence_digest(
        OccurrenceKind::Tool,
        "[tool call] Read",
        "Read",
        Some(&input),
    );
    assert_eq!(
        capture::event_id::occurrence_event_id("claude-code:s", &digest, 2),
        format!("claude-code:s:occ:{digest}:2")
    );
    Ok(())
}

#[test]
fn number_spelling_follows_the_recorded_hashes() -> Result<(), capture::json::JsonError> {
    let cases = [
        (r#"{"a":1.0}"#, r#"{"a":1}"#),
        (r#"{"a":0.1}"#, r#"{"a":0.1}"#),
        (r#"{"a":1e21}"#, r#"{"a":1e+21}"#),
        (r#"{"a":1.5e-7}"#, r#"{"a":1.5e-7}"#),
        (r#"{"a":1e-7}"#, r#"{"a":1e-7}"#),
        (r#"{"a":1e20}"#, r#"{"a":100000000000000000000}"#),
        (
            r#"{"a":12345678901234567890}"#,
            r#"{"a":12345678901234567000}"#,
        ),
        (r#"{"a":-0.0}"#, r#"{"a":0}"#),
        (r#"{"b":1,"2":2,"a":3}"#, r#"{"2":2,"b":1,"a":3}"#),
        (
            r#"{"s":"quote\"back\\slash"}"#,
            r#"{"s":"quote\"back\\slash"}"#,
        ),
        (
            r#"{"a":[1,{"b":null},true]}"#,
            r#"{"a":[1,{"b":null},true]}"#,
        ),
    ];
    for (input, expected) in cases {
        let parsed = arguments(input)?;
        assert_eq!(
            capture::event_id::arguments_spelling(&parsed),
            expected,
            "spelling of {input}"
        );
    }
    Ok(())
}
