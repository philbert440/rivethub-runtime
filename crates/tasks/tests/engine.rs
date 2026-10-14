#[expect(
    dead_code,
    reason = "each test binary includes the shared harness and uses a different subset"
)]
#[expect(
    clippy::expect_used,
    reason = "a shared harness reports setup failure by panicking; the message names the step"
)]
mod common;

use std::sync::Arc;
use std::time::Duration;

use common::RedirectKind;
use common::Script;
use common::deadline;
use common::engine;
use common::engine_with;
use common::next_body;
use common::posted;
use common::request;
use common::settings;
use common::spec;
use common::task;
use kernel::queue::FixedJob;
use kernel::queue::JobKeyMode;
use kernel::queue::JobName;
use tasks::ClaimSelector;
use tasks::Error;
use tasks::HeartbeatSpec;
use tasks::NewJob;
use tasks::RunOutcome;
use tasks::TaskJobPayload;
use tasks::TaskJobs;
use tasks::TaskOrigin;
use tasks::TaskStore;
use tokio_util::sync::CancellationToken;
use wire::mesh::MeshNodeStatus;
use wire::task::TaskStatus;

fn script(text: &'static str) -> Script {
    Script::reply(text)
}

#[tokio::test]
async fn a_chat_task_posts_the_goal_and_completes() {
    let (stub, mut rx) = common::chat_stub(script("done")).await;
    let engine = engine(common::base_url(stub.addr));
    let id = engine.submit(task("ship it")).await.expect("submit");
    let outcome = engine.run_one().await.expect("run");
    assert_eq!(outcome, RunOutcome::Completed(id));
    let recorded = next_body(&mut rx);
    assert_eq!(
        recorded.path,
        format!("/api/sessions/task:{id}/messages?wait=1")
    );
    let posted = posted(&recorded.body);
    assert_eq!(posted.text, "ship it");
    assert_eq!(posted.agent.as_deref(), Some("worker"));
    assert_eq!(posted.user_id, None);
    let loaded = engine.load(id).await.expect("load");
    assert_eq!(loaded.status, TaskStatus::Completed);
    assert_eq!(
        loaded.session_key.as_deref(),
        Some(format!("task:{id}")).as_deref()
    );
    assert_eq!(loaded.result.expect("result").summary.as_str(), "done");
    assert!(matches!(
        engine.run_one().await.expect("idle"),
        RunOutcome::Idle
    ));
    stub.close().await;
}

#[tokio::test]
async fn an_appended_system_prompt_is_posted_after_the_agent_prompt() {
    let (stub, mut rx) = common::chat_stub(script("done")).await;
    let base = common::base_url(stub.addr);
    let mut directory = common::directory(base.clone());
    let mut agent = common::plain_agent();
    agent.system_prompt = Some("base".to_string());
    directory.local_agent("worker", agent);
    let (engine, _) = engine_with(directory, settings(None, std::collections::BTreeMap::new()));
    let mut created = request("ship it");
    created.spec = Some(spec("sess-1", Some("extra"), None));
    let id = engine
        .submit(tasks::NewTask {
            request: created,
            origin: TaskOrigin::Chat,
        })
        .await
        .expect("submit");
    engine.run_one().await.expect("run");
    let posted = posted(&next_body(&mut rx).body);
    assert_eq!(posted.system_prompt.as_deref(), Some("base\nextra"));
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Completed
    );
    stub.close().await;
}

#[tokio::test]
async fn steer_replaces_the_goal_until_the_task_is_terminal() {
    let (stub, mut rx) = common::chat_stub(script("done")).await;
    let engine = engine(common::base_url(stub.addr));
    let id = engine.submit(task("ship it")).await.expect("submit");
    engine.steer(id, "more").await.expect("steer");
    assert_eq!(
        engine
            .load(id)
            .await
            .expect("load")
            .pending_message
            .as_deref(),
        Some("more")
    );
    engine.run_one().await.expect("run");
    let posted = posted(&next_body(&mut rx).body);
    assert_eq!(posted.text, "more");
    let error = engine.steer(id, "later").await.expect_err("terminal");
    assert!(matches!(
        error,
        Error::Conflict {
            status: TaskStatus::Completed,
            action: tasks::ConflictAction::Steer,
            ..
        }
    ));
    stub.close().await;
}

#[tokio::test]
async fn a_terminal_task_cannot_be_killed() {
    let (stub, _rx) = common::chat_stub(script("done")).await;
    let engine = engine(common::base_url(stub.addr));
    let id = engine.submit(task("ship it")).await.expect("submit");
    engine.run_one().await.expect("run");
    let error = engine.kill(id).await.expect_err("terminal");
    assert!(matches!(
        error,
        Error::Conflict {
            status: TaskStatus::Completed,
            action: tasks::ConflictAction::Kill,
            ..
        }
    ));
    stub.close().await;
}

#[tokio::test]
async fn an_enqueue_failure_marks_the_task_failed() {
    let base = url::Url::parse("http://127.0.0.1/").expect("url");
    let (engine, memory) = engine_with(
        common::directory(base),
        settings(None, std::collections::BTreeMap::new()),
    );
    memory.fail_next_enqueue();
    let error = engine.submit(task("ship it")).await.expect_err("enqueue");
    let Error::Enqueue { id, source } = error else {
        panic!("expected enqueue error");
    };
    assert!(matches!(source.as_ref(), Error::QueueAbsent));
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Failed
    );
}

#[tokio::test]
async fn a_second_heartbeat_replaces_the_queued_one() {
    let (stub, _rx) = common::chat_stub(script("done")).await;
    let engine = engine(common::base_url(stub.addr));
    let spec = HeartbeatSpec {
        agent: "worker".to_string(),
        index: 1,
        goal: "check".to_string(),
    };
    let first = engine
        .schedule_heartbeat(HeartbeatSpec {
            agent: spec.agent.clone(),
            index: spec.index,
            goal: spec.goal.clone(),
        })
        .await
        .expect("first");
    let second = engine.schedule_heartbeat(spec).await.expect("second");
    assert_eq!(
        engine.load(first).await.expect("first").status,
        TaskStatus::Killed
    );
    assert_eq!(
        engine.load(second).await.expect("second").status,
        TaskStatus::Queued
    );
    assert_eq!(
        engine
            .load(second)
            .await
            .expect("second")
            .session_key
            .as_deref(),
        Some("heartbeat:worker:1")
    );
    assert_eq!(
        engine.run_one().await.expect("finished"),
        RunOutcome::AlreadyFinished(first)
    );
    assert_eq!(
        engine.run_one().await.expect("run"),
        RunOutcome::Completed(second)
    );
    assert_eq!(engine.run_one().await.expect("idle"), RunOutcome::Idle);
    stub.close().await;
}

#[tokio::test]
async fn an_embed_target_job_stays_unclaimed() {
    let (stub, _rx) = common::chat_stub(script("done")).await;
    let base = common::base_url(stub.addr);
    let (engine, memory) = engine_with(
        common::directory(base),
        settings(None, std::collections::BTreeMap::new()),
    );
    let embed = memory
        .enqueue(&NewJob {
            name: JobName::Fixed(FixedJob::EmbedTarget),
            payload: TaskJobPayload {
                task_id: "not-a-task".to_string(),
            },
            key: "embed-ros_messages-1".to_string(),
            key_mode: JobKeyMode::Replace,
            priority: 0,
            max_attempts: 5,
        })
        .await
        .expect("embed");
    let id = engine.submit(task("ship it")).await.expect("submit");
    assert_eq!(
        engine.run_one().await.expect("run"),
        RunOutcome::Completed(id)
    );
    let stored = memory.read(embed).await.expect("read").expect("job");
    assert_eq!(stored.attempts, 0);
    assert_eq!(stored.identifier, "embed-target");
    stub.close().await;
}

#[tokio::test]
async fn a_remote_job_is_left_for_the_other_node() {
    let base = url::Url::parse("http://127.0.0.1/").expect("url");
    let mut directory = common::directory(base.clone());
    directory.mesh_node(common::node(
        "edge",
        "127.0.0.1",
        9,
        &["worker"],
        MeshNodeStatus::Online,
    ));
    directory.set_mesh_secret(Some("mesh-secret".to_string()));
    let (engine, memory) =
        engine_with(directory, settings(None, std::collections::BTreeMap::new()));
    let mut created = request("ship it");
    created.node_affinity = Some("edge".to_string());
    let id = engine
        .submit(tasks::NewTask {
            request: created,
            origin: TaskOrigin::Chat,
        })
        .await
        .expect("submit");
    assert_eq!(engine.run_one().await.expect("idle"), RunOutcome::Idle);
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Queued
    );
    let claimed = memory
        .claim(&ClaimSelector {
            names: vec!["run-task:edge".to_string()],
            only_id: None,
        })
        .await
        .expect("claim");
    assert!(claimed.is_some());
}

#[tokio::test]
async fn serve_finishes_a_queued_task_and_returns_when_cancelled() {
    let (stub, _rx) = common::chat_stub(script("done")).await;
    let engine = Arc::new(engine(common::base_url(stub.addr)));
    let id = engine.submit(task("ship it")).await.expect("submit");
    let mut watch = engine.subscribe();
    let cancel = CancellationToken::new();
    let worker = {
        let engine = Arc::clone(&engine);
        let cancel = cancel.clone();
        tokio::spawn(async move { engine.serve(cancel).await })
    };
    tokio::time::timeout(deadline(), watch.wait_for(|finished| *finished == Some(id)))
        .await
        .expect("watch")
        .expect("receiver");
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Completed
    );
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .expect("serve joined")
        .expect("spawn")
        .expect("serve");
    stub.close().await;
}

#[tokio::test]
async fn a_kill_during_the_turn_stays_killed_after_the_reply() {
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let (stub, mut rx) = common::chat_stub(Script {
        hold: Some(std::sync::Arc::clone(&release)),
        ..script("done")
    })
    .await;
    let engine = std::sync::Arc::new(engine(common::base_url(stub.addr)));
    let id = engine.submit(task("ship it")).await.expect("submit");
    let worker = {
        let engine = std::sync::Arc::clone(&engine);
        tokio::spawn(async move { engine.run_one().await })
    };
    tokio::time::timeout(deadline(), rx.recv())
        .await
        .expect("recorded")
        .expect("request");
    let prior = engine.kill(id).await.expect("kill");
    assert_eq!(prior, TaskStatus::Running);
    release.notify_one();
    let outcome = tokio::time::timeout(deadline(), worker)
        .await
        .expect("joined")
        .expect("spawn")
        .expect("run");
    assert_eq!(outcome, RunOutcome::AlreadyFinished(id));
    let loaded = engine.load(id).await.expect("load");
    assert_eq!(loaded.status, TaskStatus::Killed);
    assert_eq!(loaded.error.as_deref(), Some("Killed by parent"));
    stub.close().await;
}

#[tokio::test]
async fn a_gateway_deadline_is_a_timeout() {
    let (stub, mut rx) = common::chat_stub(Script {
        gateway_timeout: true,
        ..script("done")
    })
    .await;
    let engine = engine(common::base_url(stub.addr));
    let id = engine.submit(task("ship it")).await.expect("submit");
    let error = engine.run_one().await.expect_err("deadline");
    assert!(matches!(error, Error::Timeout));
    let recorded = next_body(&mut rx);
    assert_eq!(
        recorded.path,
        format!("/api/sessions/task:{id}/messages?wait=1")
    );
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Timeout
    );
    stub.close().await;
}

fn running_record(id: uuid::Uuid) -> tasks::TaskRecord {
    let stamp = chrono::Utc::now() - chrono::Duration::seconds(120);
    tasks::TaskRecord {
        id,
        goal: "ship it".to_string(),
        context_refs: Vec::new(),
        acceptance_criteria: Vec::new(),
        spec: spec("sess-1", None, None),
        executor: wire::task::TaskExecutorKind::ChatLoop,
        executor_target: None,
        requested_by: None,
        agent_id: "worker".to_string(),
        origin: TaskOrigin::Chat,
        parent_task_id: None,
        chain_depth: 0,
        budget: wire::task::TaskBudget::default(),
        node_affinity: None,
        usage: None,
        status: TaskStatus::Running,
        attempt: 1,
        max_attempts: 1,
        result: None,
        error: None,
        conversation_id: None,
        session_key: None,
        pending_message: None,
        claimed_by: Some("worker-1".to_string()),
        harness_session_ids: Vec::new(),
        created_at: stamp,
        started_at: Some(stamp),
        last_heartbeat_at: Some(stamp),
        completed_at: None,
        duration_ms: None,
        eval: None,
        eval_attempt: 0,
    }
}

#[tokio::test]
async fn a_stale_running_task_fails_when_the_worker_restarts() {
    let base = url::Url::parse("http://127.0.0.1/").expect("url");
    let (engine, memory) = engine_with(
        common::directory(base),
        settings(None, std::collections::BTreeMap::new()),
    );
    let id = uuid::Uuid::now_v7();
    memory.insert(&running_record(id)).await.expect("insert");
    assert_eq!(engine.run_one().await.expect("sweep"), RunOutcome::Idle);
    let loaded = engine.load(id).await.expect("load");
    assert_eq!(loaded.status, TaskStatus::Failed);
    assert_eq!(loaded.error.as_deref(), Some("worker_restarted"));
}

#[tokio::test]
async fn chain_depth_follows_the_parent() {
    let base = url::Url::parse("http://127.0.0.1/").expect("url");
    let engine = engine(base);
    let parent = engine.submit(task("parent")).await.expect("parent");
    let mut child = request("child");
    child.parent_task_id = Some(parent.to_string());
    child.chain_depth = Some(2);
    let error = engine
        .submit(tasks::NewTask {
            request: child,
            origin: TaskOrigin::Chat,
        })
        .await
        .expect_err("depth");
    assert!(matches!(error, Error::ChainDepth { got: 2, parent: 0 }));
    let mut child = request("child");
    child.parent_task_id = Some(parent.to_string());
    child.chain_depth = Some(1);
    let id = engine
        .submit(tasks::NewTask {
            request: child,
            origin: TaskOrigin::Chat,
        })
        .await
        .expect("child");
    assert_eq!(engine.load(id).await.expect("load").chain_depth, 1);
    let mut orphan = request("orphan");
    orphan.chain_depth = Some(2);
    assert!(matches!(
        engine
            .submit(tasks::NewTask {
                request: orphan,
                origin: TaskOrigin::Chat,
            })
            .await,
        Err(Error::ChainDepth { got: 2, parent: 0 })
    ));
}

#[tokio::test]
async fn the_api_token_refuses_a_missing_or_empty_secret() {
    let base = url::Url::parse("http://127.0.0.1/").expect("url");
    let mut configured = settings(None, std::collections::BTreeMap::new());
    configured.api_token = Some("secret".to_string());
    let (engine, _) = engine_with(common::directory(base.clone()), configured);
    assert!(matches!(
        engine.submit_with_token(None, task("ship it")).await,
        Err(Error::Unauthorized)
    ));
    assert!(matches!(
        engine
            .submit_with_token(Some("nope"), task("ship it"))
            .await,
        Err(Error::Unauthorized)
    ));
    let id = engine
        .submit_with_token(Some("secret"), task("ship it"))
        .await
        .expect("authorized");
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Queued
    );
    let mut empty = settings(None, std::collections::BTreeMap::new());
    empty.api_token = Some(String::new());
    let (engine, _) = engine_with(common::directory(base), empty);
    assert!(matches!(
        engine.submit_with_token(Some(""), task("ship it")).await,
        Err(Error::Unauthorized)
    ));
}

#[tokio::test]
async fn a_followed_redirect_reaches_the_health_document() {
    let (stub, hits) = common::redirect_stub(RedirectKind::OneHop).await;
    let client = tasks::GatewayClient::new(common::base_url(stub.addr), None).expect("client");
    let health = client.healthz().await.expect("health");
    assert!(health.ok);
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    stub.close().await;
}

#[tokio::test]
async fn two_hops_on_the_same_host_are_followed() {
    let (stub, _) = common::redirect_stub(RedirectKind::TwoHop).await;
    let client = tasks::GatewayClient::new(common::base_url(stub.addr), None).expect("client");
    assert!(client.healthz().await.expect("health").ok);
    stub.close().await;
}

#[tokio::test]
async fn the_sixth_hop_is_refused() {
    let (stub, hits) = common::redirect_stub(RedirectKind::SixHop).await;
    let client = tasks::GatewayClient::new(common::base_url(stub.addr), None).expect("client");
    let error = tokio::time::timeout(deadline(), client.healthz())
        .await
        .expect("deadline")
        .expect_err("cap");
    assert!(matches!(error, Error::Redirect(_)));
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 6);
    stub.close().await;
}

#[tokio::test]
async fn a_cross_host_redirect_does_not_connect() {
    let (stub, hits) = common::redirect_stub(RedirectKind::CrossHost).await;
    let client = tasks::GatewayClient::new(common::base_url(stub.addr), None).expect("client");
    let error = client.healthz().await.expect_err("refused");
    assert!(matches!(error, Error::Redirect(_)));
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    stub.close().await;
}
