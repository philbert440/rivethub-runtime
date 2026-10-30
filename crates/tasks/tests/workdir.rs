#[expect(
    dead_code,
    reason = "each test binary includes the shared harness and uses a different subset"
)]
#[expect(
    clippy::expect_used,
    reason = "a shared harness reports setup failure by panicking; the message names the step"
)]
mod common;

use std::collections::BTreeMap;
use std::os::unix::fs::symlink;

use common::engine_with;
use common::next_body;
use common::posted;
use common::quiet;
use common::request;
use common::settings;
use common::spec;
use tasks::Error;
use tasks::LocalAgent;
use tasks::TaskOrigin;
use wire::harness::HarnessId;
use wire::stream::ThinkingLevel;
use wire::task::Effort;
use wire::task::TaskExecutorKind;
use wire::task::TaskStatus;

fn chat() -> common::Script {
    common::Script::reply("done")
}

fn created(goal: &str, working: Option<&str>) -> tasks::NewTask {
    let mut request = request(goal);
    request.spec = Some(spec("sess-1", None, working));
    tasks::NewTask {
        request,
        origin: TaskOrigin::Chat,
    }
}

#[expect(
    clippy::expect_used,
    reason = "a test helper panics when submit, run, or load fails to set up the refusal case"
)]
async fn failed(
    working: Option<&str>,
    root: Option<&std::path::Path>,
) -> (TaskStatus, Option<String>, bool) {
    let (stub, mut rx) = common::chat_stub(chat()).await;
    let base = common::base_url(stub.addr);
    let (engine, _) = engine_with(
        common::directory(base),
        settings(root.map(std::path::Path::to_path_buf), BTreeMap::new()),
    );
    let id = engine
        .submit(created("ship it", working))
        .await
        .expect("submit");
    let _error = engine.run_one().await.expect_err("refused");
    let called = rx.try_recv().is_ok();
    let loaded = engine.load(id).await.expect("load");
    stub.close().await.expect("stub shuts down");
    (loaded.status, loaded.error, called)
}

#[tokio::test]
async fn a_relative_working_directory_inside_the_root_is_posted() {
    let root = tempfile::tempdir().expect("root");
    std::fs::create_dir(root.path().join("work")).expect("work");
    let (stub, mut rx) = common::chat_stub(chat()).await;
    let base = common::base_url(stub.addr);
    let (engine, _) = engine_with(
        common::directory(base),
        settings(Some(root.path().to_path_buf()), BTreeMap::new()),
    );
    let id = engine
        .submit(created("ship it", Some("work")))
        .await
        .expect("submit");
    engine.run_one().await.expect("run");
    let posted_chat = posted(&next_body(&mut rx).body);
    assert_eq!(
        (
            posted_chat.harness,
            posted_chat.model,
            posted_chat.working_dir
        ),
        (None, None, None)
    );
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Completed
    );
    stub.close().await.expect("stub shuts down");
}

#[tokio::test]
async fn path_escapes_do_not_call_the_gateway() {
    let root = tempfile::tempdir().expect("root");
    std::fs::create_dir(root.path().join("nested")).expect("nested");
    let (status, error, called) = failed(Some("nested/.."), Some(root.path())).await;
    assert_eq!(status, TaskStatus::Failed);
    assert_eq!(error.as_deref(), Some("path escapes the files root"));
    assert!(!called);
    let parent = tempfile::tempdir().expect("parent");
    let files = parent.path().join("files");
    let sibling = parent.path().join("files-extra");
    std::fs::create_dir(&files).expect("files");
    std::fs::create_dir(&sibling).expect("sibling");
    let (status, error, called) = failed(Some(sibling.to_str().expect("utf8")), Some(&files)).await;
    assert_eq!(status, TaskStatus::Failed);
    assert_eq!(error.as_deref(), Some("path escapes the files root"));
    assert!(!called);
    let outside = parent.path().join("outside");
    std::fs::create_dir(&outside).expect("outside");
    symlink(&outside, files.join("link")).expect("link");
    let (status, error, called) = failed(Some("link"), Some(&files)).await;
    assert_eq!(status, TaskStatus::Failed);
    assert_eq!(error.as_deref(), Some("path escapes the files root"));
    assert!(!called);
}

#[tokio::test]
async fn a_missing_directory_is_not_treated_as_an_escape() {
    let root = tempfile::tempdir().expect("root");
    let (status, error, called) = failed(Some("missing"), Some(root.path())).await;
    assert_eq!(status, TaskStatus::Failed);
    assert_eq!(error.as_deref(), Some("working directory does not exist"));
    assert!(!called);
}

fn harness_directory(base: url::Url) -> tasks::Directory {
    let mut directory = common::directory(base);
    directory.local_agent(
        "worker",
        LocalAgent {
            harness: Some(HarnessId::ClaudeCode),
            model: Some("demo".to_string()),
            provider: None,
            effort: Some(Effort::Low),
            system_prompt: Some("base".to_string()),
            working_dir: None,
        },
    );
    directory
}

fn harness_task() -> tasks::NewTask {
    let mut created = request("ship it");
    created.executor = Some(TaskExecutorKind::HarnessSession);
    tasks::NewTask {
        request: created,
        origin: TaskOrigin::Chat,
    }
}

#[tokio::test]
async fn a_harness_session_without_a_files_root_fails_before_the_gateway() {
    let (stub, mut rx) = common::chat_stub(chat()).await;
    let (engine, _) = engine_with(
        harness_directory(common::base_url(stub.addr)),
        settings(None, BTreeMap::new()),
    );
    let id = engine.submit(harness_task()).await.expect("submit");
    let error = engine.run_one().await.expect_err("root");
    assert!(matches!(error, Error::FilesRootRequired));
    quiet(&mut rx);
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Failed
    );
    stub.close().await.expect("stub shuts down");
}

#[tokio::test]
async fn a_harness_session_posts_the_recorded_message_channel() {
    let root = tempfile::tempdir().expect("root");
    let (stub, mut rx) = common::chat_stub(chat()).await;
    let (engine, _) = engine_with(
        harness_directory(common::base_url(stub.addr)),
        settings(Some(root.path().to_path_buf()), BTreeMap::new()),
    );
    let id = engine.submit(harness_task()).await.expect("submit");
    engine.run_one().await.expect("run");
    let recorded = next_body(&mut rx);
    assert_eq!(
        recorded.path,
        format!("/api/sessions/task:{id}/messages?wait=1")
    );
    let posted = posted(&recorded.body);
    assert_eq!(posted.text, "ship it");
    assert_eq!(posted.user_id, None);
    assert_eq!(posted.agent.as_deref(), Some("worker"));
    assert_eq!(posted.system_prompt.as_deref(), Some("base"));
    assert_eq!(posted.harness, Some(HarnessId::ClaudeCode));
    assert_eq!(posted.model.as_deref(), Some("demo"));
    assert_eq!(posted.thinking, Some(ThinkingLevel::Low));
    let canonical = std::fs::canonicalize(root.path()).expect("canonical root");
    assert_eq!(posted.working_dir.as_deref(), canonical.to_str());
    let loaded = engine.load(id).await.expect("load");
    assert_eq!(loaded.status, TaskStatus::Completed);
    assert_eq!(loaded.harness_session_ids, Vec::<String>::new());
    quiet(&mut rx);
    stub.close().await.expect("stub shuts down");
}

#[tokio::test]
async fn an_agent_directory_outside_the_files_root_is_used_as_configured() {
    let root = tempfile::tempdir().expect("root");
    let home = tempfile::tempdir().expect("agent home");
    let (stub, mut rx) = common::chat_stub(chat()).await;
    let mut directory = common::directory(common::base_url(stub.addr));
    directory.local_agent(
        "worker",
        LocalAgent {
            harness: Some(HarnessId::ClaudeCode),
            working_dir: Some(home.path().to_str().expect("utf8").to_owned()),
            ..common::plain_agent()
        },
    );
    let (engine, _) = engine_with(
        directory,
        settings(Some(root.path().to_path_buf()), BTreeMap::new()),
    );
    let id = engine.submit(harness_task()).await.expect("submit");
    engine.run_one().await.expect("run");
    let posted = posted(&next_body(&mut rx).body);
    let canonical = std::fs::canonicalize(home.path()).expect("canonical home");
    assert_eq!(posted.working_dir.as_deref(), canonical.to_str());
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Completed
    );
    let mut requested = request("ship it");
    requested.executor = Some(TaskExecutorKind::HarnessSession);
    requested.spec = Some(spec("sess-2", None, home.path().to_str()));
    let id = engine
        .submit(tasks::NewTask {
            request: requested,
            origin: TaskOrigin::Chat,
        })
        .await
        .expect("submit");
    let error = engine.run_one().await.expect_err("escape");
    assert!(matches!(error, Error::PathEscape));
    assert_eq!(
        engine.load(id).await.expect("load").status,
        TaskStatus::Failed
    );
    quiet(&mut rx);
    stub.close().await.expect("stub shuts down");
}
