use serde_json::json;

use testkit::Json;
use testkit::Masks;
use testkit::json_eq;
use testkit::masked_eq;

fn masks() -> Masks {
    Masks {
        port: "5174".to_string(),
        hostname: Some("gold-host".to_string()),
        tmp: Some("/tmp/golden".to_string()),
    }
}

#[test]
fn masked_tokens_match_volatile_text() {
    let m = masks();
    assert!(masked_eq("{{DATE}}", "Mon, 12 Oct 2026 00:00:00 GMT", &m).expect("recorded mask"));
    assert!(!masked_eq("{{DATE}}", "not a date", &m).expect("recorded mask"));
    assert!(masked_eq("127.0.0.1:{{PORT}}", "127.0.0.1:5174", &m).expect("recorded mask"));
    assert!(!masked_eq("127.0.0.1:{{PORT}}", "127.0.0.1:9999", &m).expect("recorded mask"));
    assert!(
        masked_eq("\"id\":\"{{PTY_1}}\"", "\"id\":\"pty-0a1b2c3d\"", &m).expect("recorded mask")
    );
    assert!(
        !masked_eq("\"id\":\"{{PTY_1}}\"", "\"id\":\"pty-nothex\"", &m).expect("recorded mask")
    );
    assert!(
        masked_eq(
            "\"ids\":[\"{{PTY_1}}\",\"{{PTY_2}}\"]",
            "\"ids\":[\"pty-0a1b2c3d\",\"pty-4e5f6071\"]",
            &m
        )
        .expect("recorded mask")
    );
    assert!(
        !masked_eq(
            "\"ids\":[\"{{PTY_1}}\",\"{{PTY_2}}\"]",
            "\"ids\":[\"pty-0a1b2c3d\",\"pty-0a1b2c3d\"]",
            &m
        )
        .expect("recorded mask")
    );
    assert!(
        masked_eq("\"pids\":[{{PID_1}},{{PID_2}}]", "\"pids\":[4711,4712]", &m)
            .expect("recorded mask")
    );
    assert!(masked_eq("{{TMP}}/state", "/tmp/golden/state", &m).expect("recorded mask"));
    assert!(
        masked_eq("\"name\":\"{{HOSTNAME}}\"", "\"name\":\"gold-host\"", &m)
            .expect("recorded mask")
    );
    assert!(
        masked_eq("\"since\":{{EPOCH_MS}}", "\"since\":1791656243919", &m).expect("recorded mask")
    );
    assert!(!masked_eq("\"since\":{{EPOCH_MS}}", "\"since\":17600001", &m).expect("recorded mask"));
    assert!(masked_eq("<ts> done", "2026-11-01T09:00:00.123Z done", &m).expect("recorded mask"));
    assert!(masked_eq("{{NODE_VERSION}}", "v22.22.2", &m).expect("recorded mask"));
    assert!(
        masked_eq(
            "\"key\":\"1791656243919-30e4c5d70\"",
            "\"key\":\"1791656243919-30e4c5d70\"",
            &m
        )
        .expect("recorded mask")
    );
    assert!(
        masked_eq(
            "\"id\":\"aitxt-0123456789abcdef01234567\"",
            "\"id\":\"aitxt-0123456789abcdef01234567\"",
            &m
        )
        .expect("recorded mask")
    );
    assert!(masked_eq("no tokens here", "no tokens here", &m).expect("recorded mask"));
    assert!(!masked_eq("no tokens here", "no tokens elsewhere", &m).expect("recorded mask"));
}

#[test]
fn json_equality_compares_parsed_values_without_null_or_number_coercion() {
    let with_nulls: Json = json!({"a": 1, "b": null, "c": [1, {"x": null}]});
    let reordered: Json = json!({"c": [1.0, {}], "a": 1.0});
    assert!(!json_eq(&with_nulls, &reordered));
    assert!(json_eq(&json!({"a": 1, "b": 2}), &json!({"b": 2, "a": 1})));
    let different: Json = json!({"a": 1, "d": 2});
    assert!(!json_eq(&with_nulls, &different));
    let ordered: Json = json!([1, 2]);
    let reversed: Json = json!([2, 1]);
    assert!(!json_eq(&ordered, &reversed));
}

#[test]
fn large_json_integers_are_not_rounded_for_comparison() {
    let first: Json = serde_json::from_str("9007199254740992").expect("integer");
    let second: Json = serde_json::from_str("9007199254740993").expect("integer");
    assert!(!json_eq(&first, &second));
    assert!(json_eq(&json!({"present": null}), &json!({})));
}
