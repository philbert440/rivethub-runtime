use std::sync::Arc;

use axum::http::StatusCode;
use gateway::routes::v1::Service;
use tokio_util::sync::CancellationToken;
use wire::gateway::v1::{
    Completion, CompletionChunk, Conversation, Conversations, ErrorResponse, Models,
};
use wire::message::Role;
use wire::stream::openai::{FinishReason, Usage};

use super::support::{Harness, REQUEST, bytes, chunks, json, request};

#[tokio::test]
async fn missing_turn_runner_refuses_before_creating_a_conversation() {
    let harness = Harness::new(chunks()).await;
    let dir = tempfile::tempdir().expect("scratch directory");
    let service = Service::open(
        "test-model".to_owned(),
        &dir.path().join("conversations"),
        CancellationToken::new(),
        std::collections::BTreeMap::new(),
    )
    .await
    .expect("empty roster opens");
    let gateway = Arc::new(gateway::Gateway::new(
        Arc::new(testkit::FakeMemoryStore::default()),
        harness.gateway.settings().clone(),
    ));
    let response = request(
        service.router(&gateway, false),
        "POST",
        "/v1/chat/completions",
        REQUEST,
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(harness.runner.turns.lock().is_empty());
    let records: Conversations = json(
        request(
            service.router(&gateway, false),
            "GET",
            "/api/conversations",
            "",
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(records.conversations, Vec::new());
    service.shutdown().await;
    harness.close().await;
}

#[tokio::test]
async fn completion_runs_one_injected_turn_and_persists_it() {
    let harness = Harness::new(chunks()).await;
    let response = request(harness.app(), "POST", "/v1/chat/completions", REQUEST, &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let conversation = response
        .headers()
        .get("x-rivet-conversation")
        .expect("conversation id")
        .to_str()
        .expect("ASCII id")
        .to_owned();
    let completion: Completion = json(response).await;
    assert!(completion.id.starts_with("chatcmpl-"));
    assert!(completion.created > 0);
    assert_eq!(completion.model, "test-model");
    let [choice] = completion.choices.as_slice() else {
        panic!("one choice")
    };
    assert_eq!(choice.index, 0);
    assert_eq!(choice.finish_reason, FinishReason::Stop);
    assert_eq!(choice.message.role, Role::Assistant);
    assert_eq!(choice.message.content.as_deref(), Some("Hello 🌍"));
    assert_eq!(
        choice.message.reasoning_content.as_deref(),
        Some("consider")
    );
    assert_eq!(
        completion.usage,
        Some(Usage {
            prompt_tokens: 3,
            completion_tokens: 2,
            total_tokens: 5,
            prompt_tokens_details: None,
            completion_tokens_details: None
        })
    );
    let record: Conversation = json(
        request(
            harness.app(),
            "GET",
            &format!("/api/conversations/{conversation}"),
            "",
            &[],
        )
        .await,
    )
    .await;
    assert_eq!(record.id, conversation);
    assert_eq!(record.messages.len(), 2);
    assert_eq!(record.messages.last(), Some(&choice.message));
    {
        let turns = std::mem::take(&mut *harness.runner.turns.lock());
        let [turn] = turns.as_slice() else {
            panic!("one turn")
        };
        assert_eq!(turn.request.model, "test-model");
        let [message] = turn.request.messages.as_slice() else {
            panic!("one user message")
        };
        assert_eq!(message.role, Role::User);
        assert_eq!(
            message.content,
            wire::message::MessageContent::Text("Hi".to_owned())
        );
        assert_eq!(
            turn.context.session_id.as_deref(),
            Some(conversation.as_str())
        );
        assert_eq!(turn.context.agent_id.as_deref(), Some("test-model"));
        assert_eq!(turn.tool_context.agent_id.as_deref(), Some("test-model"));
        assert_eq!(turn.limits.steps, 1);
        assert!(turn.authorized_tools.is_empty());
        assert_eq!(turn.request.tools, Vec::new());
    }
    harness.close().await;
}

#[tokio::test]
async fn streaming_emits_role_deltas_finish_usage_and_done() {
    let harness = Harness::new(chunks()).await;
    let input = r#"{"model":"test-model","messages":[{"role":"user","content":"Hi"}],"stream":true,"stream_options":{"include_usage":true}}"#;
    let response = request(harness.app(), "POST", "/v1/chat/completions", input, &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").expect("SSE type"),
        "text/event-stream"
    );
    let body = String::from_utf8(bytes(response).await.to_vec()).expect("UTF-8");
    let data: Vec<_> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .collect();
    assert_eq!(data.last(), Some(&"[DONE]"));
    let records: Vec<CompletionChunk> = data
        .iter()
        .take(data.len() - 1)
        .map(|data| serde_json::from_str(data).expect("chunk"))
        .collect();
    let [role, reasoning, text, finish, usage] = records.as_slice() else {
        panic!("five typed chunks: {records:?}")
    };
    assert!(records.iter().all(|record| record.id == role.id
        && record.created == role.created
        && record.model == "test-model"));
    assert_eq!(
        role.choices.first().expect("role").delta.role,
        Some(Role::Assistant)
    );
    assert_eq!(
        reasoning
            .choices
            .first()
            .expect("reasoning")
            .delta
            .reasoning_content
            .as_deref(),
        Some("consider")
    );
    assert_eq!(
        text.choices.first().expect("text").delta.content.as_deref(),
        Some("Hello 🌍")
    );
    assert_eq!(
        finish.choices.first().expect("finish").finish_reason,
        Some(FinishReason::Stop)
    );
    assert_eq!(usage.choices, Vec::new());
    assert_eq!(usage.usage.as_ref().expect("usage").total_tokens, 5);
    harness.close().await;
}

#[tokio::test]
async fn models_list_configured_agents_and_invalid_requests_are_typed_errors() {
    let harness = Harness::new(chunks()).await;
    let response = request(harness.app(), "GET", "/v1/models", "", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let models: Models = json(response).await;
    let [model] = models.data.as_slice() else {
        panic!("one model")
    };
    assert_eq!(model.id, "test-model");
    assert_eq!(model.created, 0);
    assert_eq!(model.owned_by, "rivetos");
    for body in [
        "{",
        "[]",
        r#"{"model":3,"messages":[]}"#,
        r#"{"model":"x","messages":[]}"#,
        r#"{"model":"x","messages":[{"role":"user","content":true}]}"#,
        r#"{"model":"x","messages":[{"role":"user","content":"x"}],"max_tokens":0}"#,
        r#"{"model":"x","messages":[{"role":"user","content":"x"}],"stream":"false"}"#,
    ] {
        let response = request(harness.app(), "POST", "/v1/chat/completions", body, &[]).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        let error: ErrorResponse = json(response).await;
        assert_ne!(error.error.message, "");
    }
    let records: Conversations =
        json(request(harness.app(), "GET", "/api/conversations", "", &[]).await).await;
    assert_eq!(records.conversations, Vec::new());
    assert_eq!(
        request(harness.app(), "DELETE", "/v1/models", "", &[])
            .await
            .status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
    assert_eq!(
        request(harness.app(), "GET", "/v1/unknown", "", &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    harness.close().await;
}

#[tokio::test]
async fn provider_failure_does_not_report_success_or_persist_an_assistant_message() {
    let harness = Harness::new(vec![kernel::model::ModelChunk::Text {
        delta: "partial".to_owned(),
    }])
    .await;
    let response = request(harness.app(), "POST", "/v1/chat/completions", REQUEST, &[]).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let error: ErrorResponse = json(response).await;
    assert_eq!(error.error.message, "agent turn failed");
    let records: Conversations =
        json(request(harness.app(), "GET", "/api/conversations", "", &[]).await).await;
    assert!(
        records
            .conversations
            .iter()
            .all(|record| record.messages.is_empty())
    );
    harness.close().await;
}

#[tokio::test]
async fn streamed_failure_has_an_error_frame_without_a_success_terminator() {
    let harness = Harness::new(vec![kernel::model::ModelChunk::Text {
        delta: "partial".to_owned(),
    }])
    .await;
    let response = request(
        harness.app(),
        "POST",
        "/v1/chat/completions",
        r#"{"model":"test-model","messages":[{"role":"user","content":"Hi"}],"stream":true}"#,
        &[],
    )
    .await;
    let body = String::from_utf8(bytes(response).await.to_vec()).expect("UTF-8");
    assert!(!body.contains("[DONE]"));
    let last = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .next_back()
        .expect("error frame");
    let error: ErrorResponse = serde_json::from_str(last).expect("typed stream error");
    assert_eq!(error.error.message, "agent turn failed");
    harness.close().await;
}

#[tokio::test]
async fn oversized_bodies_and_usage_overflow_fail_without_saving_messages() {
    let harness = Harness::new(vec![
        kernel::model::ModelChunk::Usage {
            usage: wire::stream::LlmUsage {
                prompt_tokens: u64::MAX,
                completion_tokens: 1,
                reasoning_tokens: None,
                cached_tokens: None,
                cache_creation_tokens: None,
                cache_read_tokens: None,
            },
        },
        kernel::model::ModelChunk::Done,
    ])
    .await;
    let oversized = " ".repeat(256 * 1024 + 1);
    assert_eq!(
        request(
            harness.app(),
            "POST",
            "/v1/chat/completions",
            &oversized,
            &[]
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        request(harness.app(), "POST", "/v1/chat/completions", REQUEST, &[])
            .await
            .status(),
        StatusCode::INTERNAL_SERVER_ERROR
    );
    let records: Conversations =
        json(request(harness.app(), "GET", "/api/conversations", "", &[]).await).await;
    assert!(
        records
            .conversations
            .iter()
            .all(|record| record.messages.is_empty())
    );
    harness.close().await;
}
