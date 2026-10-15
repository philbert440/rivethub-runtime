use std::collections::BTreeMap;

use serde_json::json;
use sqlx::PgPool;

pub(super) fn exclusions() -> Result<BTreeMap<String, String>, serde_json::Error> {
    serde_json::from_str(include_str!("../fixtures/capture-skipped.json"))
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn tool_arguments_keep_u64_precision(pool: PgPool) -> super::TestResult {
    let arguments = json!({"big": 12_345_678_901_234_567_890_u64, "timeout": 1.0});
    let payload = json!({
        "hook_event_name": "PostToolUse",
        "session_id": "integer-preservation",
        "tool_name": "Bash",
        "tool_input": arguments,
        "tool_response": "done"
    });
    super::ingest_hook_event(&pool, &payload, &json!({})).await?;
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT tool_args FROM rh_messages WHERE tool_name = 'Bash'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(stored, arguments);
    Ok(())
}

#[test]
fn title_and_content_caps_keep_whole_characters() {
    let title = format!("{}😀", "a".repeat(119));
    assert_eq!(super::title_of(&format!("{title}tail")), title);
    let content = format!("{}😀", "p".repeat(15_999));
    assert_eq!(super::capped(&content), content);
    assert_eq!(
        super::capped(&format!("{content}tail")),
        format!("{content}\n…[truncated]")
    );
}
