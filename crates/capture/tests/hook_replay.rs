use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use parking_lot::Mutex;
use testkit::HookFile;
use testkit::HookOutcome;
use testkit::HookPost;
use testkit::capture::entries;
use wire::capture::CaptureResult;

fn goldens() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

#[derive(Clone)]
struct FakeTransport {
    posts: Arc<Mutex<Vec<String>>>,
}

impl FakeTransport {
    fn new() -> Self {
        Self {
            posts: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl capture::writer::CaptureTransport for FakeTransport {
    async fn post(&self, body: String) -> Result<CaptureResult, capture::writer::PostError> {
        let inserted = serde_json::from_str::<testkit::Json>(&body)
            .ok()
            .and_then(|value| {
                value
                    .get("messages")
                    .and_then(testkit::Json::as_array)
                    .map(Vec::len)
            })
            .unwrap_or(0);
        self.posts.lock().push(body);
        Ok(CaptureResult {
            ok: true,
            conversation_id: "conv-1".to_owned(),
            inserted: u64::try_from(inserted).unwrap_or(0),
            skipped: 0,
        })
    }
}

const NOW_TEXT: &str = "2026-10-10T18:17:23.937Z";

fn prepare_store(root: &Path) -> std::io::Result<PathBuf> {
    let store = root.join("store").join("projects").join("-work-demo");
    std::fs::create_dir_all(&store)?;
    let target = store.join("33333333-3333-3333-3333-333333333333.jsonl");
    std::fs::copy(
        goldens().join("capture/claude-hook/transcript.jsonl"),
        &target,
    )?;
    Ok(target)
}

fn pretty(bodies: &[String]) -> String {
    bodies
        .iter()
        .map(|body| {
            serde_json::from_str::<testkit::Json>(body).map_or_else(
                |_| body.clone(),
                |value| serde_json::to_string_pretty(&value).unwrap_or_else(|_| body.clone()),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn pretty_files(files: &[HookFile]) -> String {
    files
        .iter()
        .map(|file| format!("{} | {} | {:?}", file.path, file.mode, file.content))
        .collect::<Vec<_>>()
        .join("\n")
}

struct EntryRun {
    outcome: HookOutcome,
    exit: i64,
    stderr: String,
}

async fn run_entry(
    entry: &testkit::capture::EntryRecord,
    transcript_text: &str,
    now: DateTime<Utc>,
) -> Result<EntryRun, Box<dyn std::error::Error>> {
    let inner = tempfile::tempdir()?;
    let state_dir = inner.path().to_path_buf();
    let transport = FakeTransport::new();
    let spool = capture::spool::CaptureSpool::new(state_dir.join("spool"));
    let writer = capture::writer::CaptureWriter::new(transport.clone(), spool, 768 * 1024, None);
    let hook = capture::hook::ClaudeHook::new(writer, state_dir.clone());
    let stdin = entry.stdin.replace("__TRANSCRIPT__", transcript_text);
    let run = hook.run(&stdin, now, "1791656243919-30e4c5d70").await;
    let posts = transport
        .posts
        .lock()
        .iter()
        .map(|body| HookPost {
            url: "/api/capture".to_owned(),
            content_type: "application/json".to_owned(),
            body: body.replace(transcript_text, "__TRANSCRIPT__"),
        })
        .collect();
    let log_path = state_dir.join(".rivetos/claude-capture.log");
    let log_content = std::fs::read_to_string(&log_path).unwrap_or_default();
    let files = vec![HookFile {
        path: ".rivetos/claude-capture.log".to_owned(),
        mode: "644".to_owned(),
        content: log_content.replace(transcript_text, "__TRANSCRIPT__"),
    }];
    Ok(EntryRun {
        outcome: HookOutcome {
            exit: run.exit,
            stdout: String::new(),
            stderr: run.stderr.clone(),
            posts,
            files,
        },
        exit: run.exit,
        stderr: run.stderr,
    })
}

async fn run_replay() -> Result<(), Box<dyn std::error::Error>> {
    let recording = goldens().join("capture/claude-hook/hook-ts.json");
    let session_entries = entries(&recording)?;
    let outer = tempfile::tempdir()?;
    let transcript = prepare_store(outer.path())?;
    let transcript_text = transcript.to_string_lossy().to_string();
    let now: DateTime<Utc> = NOW_TEXT.parse()?;
    let failures = Mutex::new(Vec::new());
    for entry in &session_entries {
        let run = run_entry(entry, &transcript_text, now).await?;
        let outcome = run.outcome.clone();
        let outcome = testkit::capture::replay(entry, |_, _| outcome.clone());
        if let Err(mismatch) = outcome {
            let detail = format!(
                "{}: recorded posts:\n{}\nactual posts:\n{}\nrecorded files:\n{}\nactual files:\n{}\nstderr: {}\nexit: {}",
                mismatch.detail,
                pretty(
                    &entry
                        .posts
                        .iter()
                        .map(|p| p.body.clone())
                        .collect::<Vec<_>>()
                ),
                pretty(
                    &run.outcome
                        .posts
                        .iter()
                        .map(|p| p.body.clone())
                        .collect::<Vec<_>>()
                ),
                entry
                    .files
                    .iter()
                    .map(|f| format!("{} | {} | {}", f.path, f.mode, f.content))
                    .collect::<Vec<_>>()
                    .join("\n"),
                pretty_files(&run.outcome.files),
                run.stderr,
                run.exit,
            );
            failures.lock().push(format!("{}: {}", mismatch.id, detail));
        }
    }
    let collected = failures.lock().join("\n");
    if collected.is_empty() {
        Ok(())
    } else {
        Err(format!("replay failure(s):\n{collected}").into())
    }
}

async fn malformed_payloads_exit_cleanly() -> Result<(), Box<dyn std::error::Error>> {
    let inner = tempfile::tempdir()?;
    let state_dir = inner.path().to_path_buf();
    let transport = FakeTransport::new();
    let spool = capture::spool::CaptureSpool::new(state_dir.join("spool"));
    let writer = capture::writer::CaptureWriter::new(transport.clone(), spool, 768 * 1024, None);
    let hook = capture::hook::ClaudeHook::new(writer, state_dir);
    let now: DateTime<Utc> = NOW_TEXT.parse()?;
    for stdin in ["{", "{}", "", "{\"hook_event_name\":\"UserPromptSubmit\"}"] {
        let run = hook.run(stdin, now, "1791656243919-30e4c5d70").await;
        assert_eq!(run.exit, 0, "stdin {stdin}");
        assert_eq!(run.stderr, "", "stdin {stdin}");
        assert!(transport.posts.lock().is_empty(), "stdin {stdin}");
    }
    Ok(())
}

async fn missing_prompt_records_the_fallback_title() -> Result<(), Box<dyn std::error::Error>> {
    let inner = tempfile::tempdir()?;
    let state_dir = inner.path().to_path_buf();
    let transport = FakeTransport::new();
    let spool = capture::spool::CaptureSpool::new(state_dir.join("spool"));
    let writer = capture::writer::CaptureWriter::new(transport.clone(), spool, 768 * 1024, None);
    let hook = capture::hook::ClaudeHook::new(writer, state_dir);
    let now: DateTime<Utc> = NOW_TEXT.parse()?;
    let stdin = r#"{"session_id":"33333333-3333-3333-3333-333333333333","hook_event_name":"UserPromptSubmit"}"#;
    let run = hook.run(stdin, now, "1791656243919-30e4c5d70").await;
    assert_eq!(run.exit, 0);
    let posts = transport.posts.lock().clone();
    let body = posts.first().ok_or("expected one post")?;
    let parsed: testkit::Json = serde_json::from_str(body)?;
    assert_eq!(
        parsed.get("title").and_then(testkit::Json::as_str),
        Some("Claude Code session")
    );
    Ok(())
}

#[test]
fn claude_hook_replays_recorded_batches() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(run_replay())?;
    runtime.block_on(malformed_payloads_exit_cleanly())?;
    runtime.block_on(missing_prompt_records_the_fallback_title())
}
