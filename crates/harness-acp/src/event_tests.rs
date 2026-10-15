use agent_client_protocol::schema::v1 as acp;
use kernel::harness::SessionEvent;

use crate::error::AcpError;
use crate::events::{permission_request, session_event};

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
