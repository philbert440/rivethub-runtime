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
        let parsed: testkit::Json =
            serde_json::from_str(&body).map_err(|error| capture::writer::PostError::Network {
                message: format!("the hook posted a body that is not JSON: {error}"),
            })?;
        let inserted = parsed
            .get("messages")
            .and_then(testkit::Json::as_array)
            .map(Vec::len)
            .ok_or_else(|| capture::writer::PostError::Network {
                message: "the posted batch body carries no messages array".to_owned(),
            })?;
        self.posts.lock().push(body);
        Ok(CaptureResult {
            ok: true,
            conversation_id: "conv-1".to_owned(),
            inserted: inserted as u64,
            skipped: 0,
        })
    }
}

const NOW_TEXT: &str = "2026-10-10T18:17:23.937Z";

fn hook_in(
    home: &Path,
    transport: &FakeTransport,
) -> Result<capture::hook::ClaudeHook<FakeTransport>, config::ConfigError> {
    let env = config::Environment::from_vars([("HOME".to_owned(), home.display().to_string())]);
    let spool = capture::spool::CaptureSpool::new(home.join("spool"));
    let writer = capture::writer::CaptureWriter::new(
        transport.clone(),
        spool,
        std::num::NonZeroUsize::new(768 * 1024),
        None,
    );
    Ok(capture::hook::ClaudeHook::new(
        writer,
        &config::Paths::new(&env)?,
    ))
}

fn transcript_char_lengths() -> Result<Vec<(String, usize)>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(testkit::recording("capture/claude-hook/transcript.jsonl"))?;
    Ok(capture::transcript::parse_lines(&text)
        .lines
        .iter()
        .filter_map(|line| Some((line.uuid.clone()?, line.text()?.chars().count())))
        .collect())
}

fn as_rust_behaviour(
    entry: &testkit::capture::EntryRecord,
    char_lengths: &[(String, usize)],
) -> Result<testkit::capture::EntryRecord, Box<dyn std::error::Error>> {
    let mut entry = entry.clone();
    for file in &mut entry.files {
        if let Some(rest) = file.path.strip_prefix(".rivetos/") {
            file.path = format!(".rivethub/{rest}");
        }
    }
    for post in &mut entry.posts {
        let mut body: testkit::Json = serde_json::from_str(&post.body)?;
        let messages = body
            .get_mut("messages")
            .and_then(testkit::Json::as_array_mut)
            .into_iter()
            .flatten();
        for message in messages {
            let Some(metadata) = message.get_mut("metadata") else {
                continue;
            };
            let uuid = metadata
                .get("uuid")
                .and_then(testkit::Json::as_str)
                .map(str::to_owned);
            let Some(recorded_length) = metadata.get_mut("full_content_length") else {
                continue;
            };
            let uuid = uuid.ok_or("capped message has no uuid")?;
            let (_, chars) = char_lengths
                .iter()
                .find(|(line, _)| *line == uuid)
                .ok_or("capped message has no transcript line")?;
            *recorded_length = testkit::Json::from(*chars);
        }
        post.body = serde_json::to_string(&body)?;
    }
    Ok(entry)
}

fn prepare_store(root: &Path) -> std::io::Result<PathBuf> {
    let store = root.join("store").join("projects").join("-work-demo");
    std::fs::create_dir_all(&store)?;
    let target = store.join("33333333-3333-3333-3333-333333333333.jsonl");
    std::fs::copy(
        testkit::recording("capture/claude-hook/transcript.jsonl"),
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
    let transport = FakeTransport::new();
    let hook = hook_in(inner.path(), &transport)?;
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
    let log_content = std::fs::read_to_string(hook.log_file())?;
    let files = vec![HookFile {
        path: hook
            .log_file()
            .strip_prefix(inner.path())?
            .display()
            .to_string(),
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

#[tokio::test]
async fn claude_hook_replays_recorded_batches() -> Result<(), Box<dyn std::error::Error>> {
    let recording = testkit::recording("capture/claude-hook/hook-ts.json");
    let session_entries = entries(&recording)?;
    let skipped: std::collections::BTreeMap<String, String> =
        serde_json::from_str(include_str!("../fixtures/hook-skipped.json"))?;
    let outer = tempfile::tempdir()?;
    let transcript = prepare_store(outer.path())?;
    let transcript_text = transcript.to_string_lossy().to_string();
    let now: DateTime<Utc> = NOW_TEXT.parse()?;
    let failures = Mutex::new(Vec::new());
    let char_lengths = transcript_char_lengths()?;
    for recorded in &session_entries {
        if skipped.contains_key(&format!("hook/{}", recorded.index)) {
            continue;
        }
        let entry = &as_rust_behaviour(recorded, &char_lengths)?;
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

#[tokio::test]
async fn malformed_payloads_exit_cleanly() -> Result<(), Box<dyn std::error::Error>> {
    let inner = tempfile::tempdir()?;
    let transport = FakeTransport::new();
    let hook = hook_in(inner.path(), &transport)?;
    let now: DateTime<Utc> = NOW_TEXT.parse()?;
    for stdin in ["{", "{}", "", "{\"hook_event_name\":\"UserPromptSubmit\"}"] {
        let run = hook.run(stdin, now, "1791656243919-30e4c5d70").await;
        assert_eq!(run.exit, 0, "stdin {stdin}");
        assert_eq!(run.stderr, "", "stdin {stdin}");
        assert!(transport.posts.lock().is_empty(), "stdin {stdin}");
    }
    Ok(())
}

#[tokio::test]
async fn missing_prompt_records_the_fallback_title() -> Result<(), Box<dyn std::error::Error>> {
    let inner = tempfile::tempdir()?;
    let transport = FakeTransport::new();
    let hook = hook_in(inner.path(), &transport)?;
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
    let content = parsed
        .get("messages")
        .and_then(testkit::Json::as_array)
        .and_then(|messages| messages.first())
        .and_then(|message| message.get("content"))
        .and_then(testkit::Json::as_str);
    assert_eq!(content, Some(""));
    Ok(())
}

#[tokio::test]
async fn a_failed_log_write_does_not_fail_the_run() -> Result<(), Box<dyn std::error::Error>> {
    let inner = tempfile::tempdir()?;
    std::fs::write(inner.path().join(".rivethub"), b"not a directory")?;
    let transport = FakeTransport::new();
    let hook = hook_in(inner.path(), &transport)?;
    let now: DateTime<Utc> = NOW_TEXT.parse()?;
    let stdin = r#"{"session_id":"33333333-3333-3333-3333-333333333333","hook_event_name":"UserPromptSubmit","prompt":"list the files"}"#;
    let run = hook.run(stdin, now, "1791656243919-30e4c5d70").await;
    assert_eq!(run.exit, 0);
    assert_eq!(run.stderr, "");
    assert_eq!(transport.posts.lock().len(), 1);
    Ok(())
}

#[test]
fn hook_exclusions_name_existing_recordings() -> Result<(), Box<dyn std::error::Error>> {
    let skipped: std::collections::BTreeMap<String, String> =
        serde_json::from_str(include_str!("../fixtures/hook-skipped.json"))?;
    let rows = entries(&testkit::recording("capture/claude-hook/hook-ts.json"))?;
    assert_eq!(skipped.len(), 3);
    for (name, reason) in skipped {
        assert_ne!(reason, "");
        assert!(
            rows.iter()
                .any(|entry| format!("hook/{}", entry.index) == name)
        );
    }
    Ok(())
}

#[tokio::test]
async fn transcript_tool_arguments_preserve_input_values() -> Result<(), Box<dyn std::error::Error>>
{
    let rows = entries(&testkit::recording("capture/claude-hook/hook-ts.json"))?;
    let outer = tempfile::tempdir()?;
    let transcript = prepare_store(outer.path())?;
    let source = std::fs::read_to_string(&transcript)?;
    let parsed = capture::transcript::parse_lines(&source);
    let expected: Vec<_> = parsed
        .lines
        .iter()
        .flat_map(capture::transcript::Line::tool_uses)
        .map(|tool| (tool.name.to_owned(), tool.input.arguments().clone()))
        .collect();
    assert_eq!(expected.len(), 1);
    let arguments = &expected.first().ok_or("missing source tool arguments")?.1;
    assert_eq!(
        arguments.get("big"),
        Some(&testkit::Json::from(12_345_678_901_234_567_890_u64))
    );
    assert_eq!(arguments.get("timeout"), Some(&testkit::Json::from(1.0)));
    for index in [3, 4, 5] {
        let entry = rows
            .iter()
            .find(|entry| entry.index == index)
            .ok_or("missing transcript case")?;
        let run = run_entry(
            entry,
            transcript.to_str().ok_or("non-UTF-8 transcript path")?,
            NOW_TEXT.parse()?,
        )
        .await?;
        assert_eq!(run.exit, 0);
        let mut actual = Vec::new();
        for post in run.outcome.posts {
            let batch: wire::capture::CaptureBatchRequest = serde_json::from_str(&post.body)?;
            for message in batch.messages {
                if let Some(arguments) = message.tool_args {
                    actual.push((
                        message.tool_name.ok_or("tool arguments require a name")?,
                        arguments,
                    ));
                }
            }
        }
        assert_eq!(actual, expected, "hook/{index}");
    }
    Ok(())
}
