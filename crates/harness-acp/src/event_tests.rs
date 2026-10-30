use agent_client_protocol::schema::v1 as acp;
use kernel::harness::SessionEvent;

use crate::error::AcpError;
use crate::events::{permission_request, session_event, turn_usage};

#[test]
fn absent_permission_titles_are_refused() {
    let request = acp::RequestPermissionRequest::new(
        "session",
        acp::ToolCallUpdate::new("call", acp::ToolCallUpdateFields::new()),
        Vec::new(),
    );
    assert!(matches!(
        permission_request("permission", &request),
        Err(AcpError::MissingPermissionTitle)
    ));
}

#[test]
fn text_is_preserved_including_empty_text_and_unicode() {
    for text in ["", "😺 café"] {
        let event = session_event(acp::SessionUpdate::AgentMessageChunk(
            acp::ContentChunk::new(acp::ContentBlock::from(text)),
        ))
        .expect("text");
        assert_eq!(
            event,
            Some(SessionEvent::AgentMessageChunk {
                text: text.to_owned()
            })
        );
    }
}

#[test]
fn media_is_not_replaced_with_empty_text() {
    let content: acp::ContentBlock =
        serde_json::from_str(r#"{"type":"image","data":"YQ==","mimeType":"image/png"}"#)
            .expect("image");
    assert!(matches!(
        session_event(acp::SessionUpdate::AgentMessageChunk(
            acp::ContentChunk::new(content)
        )),
        Err(AcpError::NonTextContent)
    ));
}

#[test]
fn missing_tool_output_and_empty_tool_output_remain_distinct() {
    let missing = session_event(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(
            "call",
            acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
        ),
    ))
    .expect("missing output");
    let present = session_event(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(
            "call",
            acp::ToolCallUpdateFields::new()
                .status(acp::ToolCallStatus::Completed)
                .content(vec![acp::ToolCallContent::from("")]),
        ),
    ))
    .expect("empty output");
    let Some(SessionEvent::ToolCallUpdate(missing)) = missing else {
        panic!("tool update")
    };
    let Some(SessionEvent::ToolCallUpdate(present)) = present else {
        panic!("tool update")
    };
    assert_eq!(missing.output, None);
    assert_eq!(present.output, Some(String::new()));
}

#[derive(serde::Serialize)]
struct CommandInput {
    command: &'static str,
}

#[test]
fn a_tool_update_carries_the_title_and_input_it_brings() {
    let raw = serde_json::to_value(CommandInput {
        command: "cargo test",
    })
    .expect("input");
    let update = session_event(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(
            "call",
            acp::ToolCallUpdateFields::new()
                .title("`cargo test`".to_owned())
                .raw_input(raw),
        ),
    ))
    .expect("tool update");
    let Some(SessionEvent::ToolCallUpdate(update)) = update else {
        panic!("tool update")
    };
    assert_eq!(update.title.as_deref(), Some("`cargo test`"));
    let command = update
        .input
        .as_ref()
        .and_then(|input| input.get("command"))
        .and_then(|value| value.as_str());
    assert_eq!(command, Some("cargo test"));
}

#[test]
fn a_tool_call_is_named_by_its_tool_when_the_agent_says_so() {
    let named = |call: acp::ToolCall| match session_event(acp::SessionUpdate::ToolCall(call)) {
        Ok(Some(SessionEvent::ToolCall(call))) => call.name,
        other => panic!("tool call: {other:?}"),
    };
    assert_eq!(
        named(acp::ToolCall::new("call", "Terminal").name("Bash".to_owned())),
        "Bash"
    );
    assert_eq!(named(acp::ToolCall::new("call", "Terminal")), "Terminal");
}

#[tokio::test]
async fn config_notifications_refresh_the_current_model() {
    let options = crate::options::SessionOptions::default();
    let (events, mut receiver) = tokio::sync::mpsc::channel(1);
    let update = acp::ConfigOptionUpdate::new(vec![
        acp::SessionConfigOption::select(
            "model",
            "Model",
            "fast",
            vec![acp::SessionConfigSelectGroup::new(
                "models",
                "Models",
                vec![
                    acp::SessionConfigSelectOption::new("fast", "Fast model"),
                    acp::SessionConfigSelectOption::new("deep", "Deep model"),
                ],
            )],
        )
        .category(acp::SessionConfigOptionCategory::Model),
    ]);
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::client::on_session_update(
            acp::SessionNotification::new(
                "session",
                acp::SessionUpdate::ConfigOptionUpdate(update),
            ),
            &events,
            &options,
        ),
    )
    .await
    .expect("notification deadline")
    .expect("notification");
    let models = options
        .models()
        .expect("models")
        .expect("advertised models");
    assert_eq!(models.first().expect("first model").label, "Fast model");
    assert_eq!(models.first().expect("first model").is_default, Some(true));
    assert_eq!(models.last().expect("last model").is_default, Some(false));
    assert!(receiver.try_recv().is_err());
}

#[test]
fn a_current_model_outside_the_advertised_choices_is_rejected() {
    let options = crate::options::SessionOptions::default();
    options.replace(vec![
        acp::SessionConfigOption::select(
            "model",
            "Model",
            "absent",
            vec![acp::SessionConfigSelectOption::new("fast", "Fast")],
        )
        .category(acp::SessionConfigOptionCategory::Model),
    ]);
    assert!(matches!(
        options.models(),
        Err(AcpError::ConfigValueRejected { .. })
    ));
}

#[test]
fn session_metadata_is_accepted_without_inventing_a_text_event() {
    let update: acp::SessionUpdate =
        serde_json::from_str(r#"{"sessionUpdate":"session_info_update","title":"Retitled"}"#)
            .expect("session info");
    assert_eq!(session_event(update).expect("accepted metadata"), None);
}

fn usage_of(result: &str) -> Option<(u64, u64)> {
    let response: acp::PromptResponse = serde_json::from_str(result).expect("prompt result");
    turn_usage(&response)
        .expect("usage")
        .map(|usage| (usage.prompt_tokens, usage.completion_tokens))
}

#[test]
fn recorded_prompt_results_yield_full_prompt_and_output_tokens() {
    let reported = r#"{"stopReason":"end_turn","usage":{"inputTokens":2,"outputTokens":4,"cachedReadTokens":10470,"cachedWriteTokens":12776,"totalTokens":23252}}"#;
    let meta = r#"{"stopReason":"end_turn","_meta":{"totalTokens":20238,"inputTokens":20185,"outputTokens":44,"usage":{"inputTokens":20185,"outputTokens":44,"totalTokens":20229,"cachedReadTokens":2176,"cacheCreationTokens":0,"reasoningTokens":43,"modelCalls":1,"numTurns":1}}}"#;
    assert_eq!(usage_of(reported), Some((23_248, 4)));
    assert_eq!(usage_of(meta), Some((20_185, 44)));
}

#[test]
fn a_prompt_result_without_usage_reports_none() {
    assert_eq!(usage_of(r#"{"stopReason":"end_turn"}"#), None);
    assert_eq!(
        usage_of(r#"{"stopReason":"end_turn","_meta":{"usage":{"modelCalls":1}}}"#),
        None
    );
}

#[test]
fn usage_meta_that_is_not_token_counts_is_refused() {
    let response: acp::PromptResponse = serde_json::from_str(
        r#"{"stopReason":"end_turn","_meta":{"usage":{"inputTokens":"many"}}}"#,
    )
    .expect("prompt result");
    assert!(matches!(
        turn_usage(&response),
        Err(AcpError::UsageMeta { .. })
    ));
}
