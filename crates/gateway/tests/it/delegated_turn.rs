#![cfg(test)]
use crate::session_support;

use std::sync::Arc;

use kernel::harness::SessionSpec;
use session_support::harness::Harness;
use session_support::{Fixture, http, user};
use wire::gateway::GatewayError;
use wire::gateway::sessions::{MessageUsage, SessionPostReply, SessionPostRequest};
use wire::harness::HarnessId;
use wire::stream::{LlmUsage, ThinkingLevel};

fn delegated(harness: HarnessId, working_dir: Option<String>) -> SessionPostRequest {
    SessionPostRequest {
        exclude_tools: None,
        text: "ship it".to_owned(),
        user_id: None,
        agent: Some("worker".to_owned()),
        thinking: Some(ThinkingLevel::High),
        system_prompt: Some("base".to_owned()),
        attachments: None,
        harness: Some(harness),
        model: Some("opus".to_owned()),
        working_dir,
    }
}

async fn post(fixture: &Fixture, body: &SessionPostRequest) -> (u16, Vec<u8>) {
    http(
        &fixture.gateway,
        "POST",
        "/api/sessions/task:abc/messages?wait=1",
        serde_json::to_vec(body).expect("body"),
        None,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_waited_turn_naming_a_harness_runs_a_harness_session_in_the_working_directory() {
    let harness = Arc::new(Harness::default());
    let fixture = Fixture::with_harness(Some(harness.clone())).await;
    let work = fixture.directory.path().join("work");
    std::fs::create_dir(&work).expect("work directory");
    harness.finish.add_permits(1);
    let body = delegated(
        HarnessId::ClaudeCode,
        Some(work.to_str().expect("utf-8").to_owned()),
    );
    let (status, reply) = post(&fixture, &body).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&reply));
    let reply: SessionPostReply = serde_json::from_slice(&reply).expect("reply");
    assert_eq!(reply.message.text, "answer");
    assert_eq!(reply.message.session_id, "task:abc");
    assert_eq!(reply.message.usage, None);
    let specs = harness.specs.lock().clone();
    let spec = specs.first().expect("one harness session");
    assert_eq!(specs.len(), 1);
    assert_eq!(
        (spec.harness, spec.model.as_deref(), spec.effort),
        (
            HarnessId::ClaudeCode,
            Some("opus"),
            Some(ThinkingLevel::High)
        )
    );
    assert_eq!(
        spec.working_dir,
        Some(std::fs::canonicalize(&work).expect("canonical"))
    );
    assert_eq!(
        harness.trace.borrow().clone(),
        vec![
            "new".to_owned(),
            "prompt:[System instructions]\nbase\n\nship it".to_owned(),
            "ended".to_owned(),
            "release".to_owned(),
        ]
    );
    let listed = fixture.service.harness_sessions(&user("owner")).sessions;
    assert_eq!(
        listed
            .iter()
            .map(|summary| summary.effort.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("high")]
    );
    fixture.gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_waited_turn_without_a_working_directory_uses_the_harness_command_directory() {
    let harness = Arc::new(Harness::default());
    let fixture = Fixture::with_harness(Some(harness.clone())).await;
    harness.finish.add_permits(1);
    let (status, _) = post(&fixture, &delegated(HarnessId::ClaudeCode, None)).await;
    assert_eq!(status, 200);
    let specs: Vec<SessionSpec> = harness.specs.lock().clone();
    assert_eq!(
        specs.first().and_then(|spec| spec.working_dir.clone()),
        Some(fixture.directory.path().to_path_buf())
    );
    fixture.gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_harness_this_node_does_not_offer_is_refused_before_any_session() {
    let harness = Arc::new(Harness::default());
    let fixture = Fixture::with_harness(Some(harness.clone())).await;
    let (status, reply) = post(&fixture, &delegated(HarnessId::Codex, None)).await;
    assert_eq!(status, 400);
    let error: GatewayError = serde_json::from_slice(&reply).expect("error");
    assert_eq!(error.error, "harness is not allowed on this node");
    assert!(harness.specs.lock().is_empty());
    fixture.gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_working_directory_outside_the_root_is_refused() {
    let harness = Arc::new(Harness::default());
    let fixture = Fixture::with_harness(Some(harness.clone())).await;
    let outside = tempfile::tempdir().expect("outside");
    let body = delegated(
        HarnessId::ClaudeCode,
        Some(outside.path().to_str().expect("utf-8").to_owned()),
    );
    let (status, _) = post(&fixture, &body).await;
    assert_eq!(status, 403);
    assert!(harness.specs.lock().is_empty());
    fixture.gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relative_working_directory_resolves_inside_the_root() {
    let harness = Arc::new(Harness::default());
    let fixture = Fixture::with_harness(Some(harness.clone())).await;
    let work = fixture.directory.path().join("work");
    std::fs::create_dir(&work).expect("work directory");
    harness.finish.add_permits(1);
    let body = delegated(HarnessId::ClaudeCode, Some("work".to_owned()));
    let (status, reply) = post(&fixture, &body).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&reply));
    let specs: Vec<SessionSpec> = harness.specs.lock().clone();
    assert_eq!(
        specs.first().and_then(|spec| spec.working_dir.clone()),
        Some(std::fs::canonicalize(&work).expect("canonical"))
    );
    fixture.gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relative_working_directory_cannot_climb_out_of_the_root() {
    let harness = Arc::new(Harness::default());
    let fixture = Fixture::with_harness(Some(harness.clone())).await;
    let body = delegated(HarnessId::ClaudeCode, Some("../elsewhere".to_owned()));
    let (status, _) = post(&fixture, &body).await;
    assert_eq!(status, 400);
    assert!(harness.specs.lock().is_empty());
    fixture.gateway.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_usage_a_harness_reports_for_the_turn_is_on_the_reply() {
    let harness = Arc::new(Harness::default());
    *harness.usage.lock() = Some(LlmUsage {
        prompt_tokens: 23_248,
        completion_tokens: 4,
        reasoning_tokens: None,
        cached_tokens: None,
        cache_creation_tokens: Some(12_776),
        cache_read_tokens: Some(10_470),
    });
    let fixture = Fixture::with_harness(Some(harness.clone())).await;
    harness.finish.add_permits(1);
    let (status, reply) = post(&fixture, &delegated(HarnessId::ClaudeCode, None)).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&reply));
    let reply: SessionPostReply = serde_json::from_slice(&reply).expect("reply");
    assert_eq!(
        reply.message.usage,
        Some(MessageUsage {
            prompt_tokens: 23_248,
            completion_tokens: 4,
            cached_tokens: 10_470,
        })
    );
    fixture.gateway.close().await;
}
