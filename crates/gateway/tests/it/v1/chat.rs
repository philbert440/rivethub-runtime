use std::collections::BTreeSet;
use std::sync::Arc;

use axum::http::StatusCode;
use gateway::tenancy::{Tenancy, UserId};
use wire::gateway::sessions::{SessionPostReply, SessionPostRequest};

use super::support::{Harness, chunks, json, request};

#[tokio::test]
async fn waited_chat_passes_task_exclusions_into_the_turn() {
    for exclusions in [
        None,
        Some(Vec::new()),
        Some(vec!["delegate_task".to_owned(), "wiki_search".to_owned()]),
    ] {
        let harness = local_harness().await;
        let body = SessionPostRequest {
            exclude_tools: exclusions.clone(),
            text: "finish the ledger".to_owned(),
            user_id: None,
            agent: Some("main".to_owned()),
            thinking: None,
            system_prompt: None,
            attachments: None,
            harness: None,
            model: None,
            working_dir: None,
        };
        let response = request(
            harness.app(),
            "POST",
            "/api/sessions/task:test/messages?wait=1",
            &serde_json::to_string(&body).expect("request"),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let reply: SessionPostReply = json(response).await;
        assert_eq!(reply.message.text, "Hello 🌍");
        {
            let turns = std::mem::take(&mut *harness.runner.turns.lock());
            let [turn] = turns.as_slice() else {
                panic!("one turn")
            };
            let expected: BTreeSet<String> = exclusions.into_iter().flatten().collect();
            assert_eq!(turn.excluded_tools, expected);
            assert!(turn.authorized_tools.is_empty());
        }
        harness.close().await;
    }
}

#[tokio::test]
async fn waited_chat_refuses_malformed_exclusions() {
    let harness = local_harness().await;
    let response = request(
        harness.app(),
        "POST",
        "/api/sessions/task:test/messages?wait=1",
        r#"{"text":"finish the ledger","excludeTools":"delegate_task"}"#,
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(harness.runner.turns.lock().is_empty());
    harness.close().await;
}

async fn local_harness() -> Harness {
    let mut harness = Harness::new(chunks()).await;
    harness.gateway = Arc::new(
        Arc::into_inner(harness.gateway)
            .expect("unshared test gateway")
            .with_tenancy(Tenancy::local(
                UserId::new("owner".to_owned()).expect("user"),
            )),
    );
    harness
}
