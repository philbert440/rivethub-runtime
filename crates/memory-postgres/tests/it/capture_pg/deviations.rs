use std::collections::BTreeMap;

use sqlx::PgPool;

pub(super) fn exclusions() -> Result<BTreeMap<String, String>, serde_json::Error> {
    serde_json::from_str(include_str!("../../fixtures/capture-skipped.json"))
}

#[derive(serde::Serialize)]
struct HookPayload<'a> {
    hook_event_name: &'a str,
    session_id: &'a str,
    tool_name: &'a str,
    tool_input: &'a wire::JsonObject,
    tool_response: &'a str,
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn tool_arguments_keep_u64_precision(pool: PgPool) -> super::TestResult {
    let arguments: wire::JsonObject =
        serde_json::from_str(r#"{"big":12345678901234567890,"timeout":1.0}"#)?;
    let payload = serde_json::to_value(HookPayload {
        hook_event_name: "PostToolUse",
        session_id: "integer-preservation",
        tool_name: "Bash",
        tool_input: &arguments,
        tool_response: "done",
    })?;
    let options = serde_json::to_value(wire::JsonObject::default())?;
    super::ingest_hook_event(&pool, &payload, &options).await?;
    let stored: sqlx::types::Json<wire::JsonObject> =
        sqlx::query_scalar("SELECT tool_args FROM rh_messages WHERE tool_name = 'Bash'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(stored.0, arguments);
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
