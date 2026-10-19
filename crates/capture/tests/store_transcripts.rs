use std::path::Path;

use kernel::transcript::TranscriptReader;
use kernel::transcript::TranscriptReaderError;
use kernel::transcript::Turn;
use wire::harness::HarnessId;
use wire::message::Role;
use wire::session_id::SessionId;

use capture::listing::StoreRoots;
use capture::listing::StoreTranscriptReader;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const SESSION: &str = "33333333-3333-3333-3333-333333333333";

fn place(root: &Path, relative: &str, fixture: &str) -> TestResult {
    let destination = root.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(testkit::recording(fixture), &destination)?;
    Ok(())
}

fn qwen_store(root: &Path) -> TestResult {
    place(
        root,
        &format!("projects/-work-demo/chats/{SESSION}.jsonl"),
        "capture/stores/qwen.jsonl",
    )
}

fn codex_store(root: &Path) -> TestResult {
    place(
        root,
        &format!("sessions/2026/10/14/rollout-2026-10-14T12-00-00-{SESSION}.jsonl"),
        "capture/stores/codex-rollout.jsonl",
    )
}

fn roles(turns: &[Turn]) -> Vec<(Role, &str)> {
    turns
        .iter()
        .map(|turn| (turn.role, turn.content.as_str()))
        .collect()
}

const DIALOGUE: [(Role, &str); 3] = [
    (Role::User, "list the files"),
    (Role::Assistant, "Running ls."),
    (Role::Tool, "a.txt"),
];

struct Stores {
    qwen: tempfile::TempDir,
    codex: tempfile::TempDir,
    reader: StoreTranscriptReader,
}

fn stores() -> TestResult<Stores> {
    let qwen = tempfile::tempdir()?;
    let codex = tempfile::tempdir()?;
    qwen_store(qwen.path())?;
    codex_store(codex.path())?;
    let reader = StoreTranscriptReader::new(StoreRoots {
        qwen: Some(qwen.path().to_path_buf()),
        codex: Some(codex.path().to_path_buf()),
        ..StoreRoots::default()
    });
    Ok(Stores {
        qwen,
        codex,
        reader,
    })
}

#[tokio::test]
async fn one_reader_serves_each_harness_from_its_own_store() -> TestResult {
    let stores = stores()?;
    for harness in [HarnessId::QwenCode, HarnessId::Codex] {
        let session = SessionId::new(harness, SESSION);
        assert_eq!(roles(&stores.reader.read(&session).await?), DIALOGUE);
        let since = "2026-10-14T12:00:01Z".parse()?;
        let tail = stores.reader.tail(&session, since).await?;
        assert_eq!(roles(&tail), [(Role::Tool, "a.txt")]);
    }
    Ok(())
}

#[tokio::test]
async fn sessions_are_listed_from_the_store_they_name() -> TestResult {
    let stores = stores()?;
    assert_eq!(
        stores.reader.sessions(stores.qwen.path()).await?,
        [SessionId::new(HarnessId::QwenCode, SESSION)]
    );
    assert_eq!(
        stores.reader.sessions(stores.codex.path()).await?,
        [SessionId::new(HarnessId::Codex, SESSION)]
    );
    let stranger = tempfile::tempdir()?;
    let refused = stores.reader.sessions(stranger.path()).await;
    assert!(
        matches!(refused, Err(TranscriptReaderError::Unavailable(_))),
        "{refused:?}"
    );
    Ok(())
}

#[tokio::test]
async fn a_harness_without_a_store_is_unavailable() -> TestResult {
    let stores = stores()?;
    for harness in [HarnessId::ClaudeCode, HarnessId::Cowork] {
        let read = stores.reader.read(&SessionId::new(harness, SESSION)).await;
        assert!(
            matches!(read, Err(TranscriptReaderError::Unavailable(_))),
            "{harness}: {read:?}"
        );
    }
    Ok(())
}
