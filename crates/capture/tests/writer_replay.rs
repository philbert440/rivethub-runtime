use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use parking_lot::Mutex;
use testkit::json_eq;
use wire::capture::CaptureResult;

fn recording(name: &str) -> String {
    testkit::recording("capture/writer")
        .join(name)
        .to_string_lossy()
        .to_string()
}

#[derive(Clone)]
struct RecordingTransport {
    posts: Arc<Mutex<Vec<String>>>,
}

impl RecordingTransport {
    fn new() -> Self {
        Self {
            posts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn respond(body: &str) -> CaptureResult {
        let messages = serde_json::from_str::<testkit::Json>(body)
            .ok()
            .and_then(|value| {
                value
                    .get("messages")
                    .and_then(testkit::Json::as_array)
                    .map(Vec::len)
            })
            .unwrap_or(0);
        CaptureResult {
            ok: true,
            conversation_id: "c-1".to_owned(),
            inserted: u64::try_from(messages).unwrap_or(0),
            skipped: 0,
        }
    }
}

#[async_trait]
impl capture::writer::CaptureTransport for RecordingTransport {
    async fn post(&self, body: String) -> Result<CaptureResult, capture::writer::PostError> {
        let response = Self::respond(&body);
        self.posts.lock().push(body);
        Ok(response)
    }
}

fn recorded_posts(name: &str) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(format!("{}.posts.json", recording(name)))?;
    let parsed: testkit::Json = serde_json::from_str(&text)?;
    let mut posts = Vec::new();
    for post in parsed.as_array().ok_or("posts file is not an array")? {
        posts.push((
            post.get("url")
                .and_then(testkit::Json::as_str)
                .unwrap_or_default()
                .to_owned(),
            post.get("body")
                .and_then(testkit::Json::as_str)
                .unwrap_or_default()
                .to_owned(),
        ));
    }
    Ok(posts)
}

fn compare(actual: &[String], recorded: &[(String, String)], label: &str) -> Vec<String> {
    let mut failures = Vec::new();
    if actual.len() != recorded.len() {
        failures.push(format!(
            "{label}: {} post(s), recorded {}",
            actual.len(),
            recorded.len()
        ));
    }
    for (index, (actual_body, (_, recorded_body))) in actual.iter().zip(recorded).enumerate() {
        let Ok(actual_value) = serde_json::from_str::<testkit::Json>(actual_body) else {
            failures.push(format!("{label} post {index}: actual body unreadable"));
            continue;
        };
        let Ok(recorded_value) = serde_json::from_str::<testkit::Json>(recorded_body) else {
            failures.push(format!("{label} post {index}: recorded body unreadable"));
            continue;
        };
        if !json_eq(&recorded_value, &actual_value) {
            failures.push(format!(
                "{label} post {index}: batch body mismatch\n    recorded: {}\n    actual:   {}",
                serde_json::to_string_pretty(&recorded_value).unwrap_or_default(),
                serde_json::to_string_pretty(&actual_value).unwrap_or_default(),
            ));
        }
    }
    failures
}

const NOW_TEXT: &str = "2026-10-10T12:00:09.000Z";

async fn run_case(
    name: &str,
    max_chunk_bytes: usize,
    redaction: Option<capture::redaction::Redaction>,
) -> Result<(), Box<dyn std::error::Error>> {
    let input = std::fs::read_to_string(format!("{}.input.json", recording(name)))?;
    let batch = capture::batch::parse_batch(&input)?;
    let recorded = recorded_posts(name)?;
    let transport = RecordingTransport::new();
    let spool_dir = tempfile::tempdir()?;
    let spool = capture::spool::CaptureSpool::new(spool_dir.path().to_path_buf());
    let writer =
        capture::writer::CaptureWriter::new(transport.clone(), spool, max_chunk_bytes, redaction);
    let now: DateTime<Utc> = NOW_TEXT.parse()?;
    let totals = writer.write(&batch, now).await?;
    let posts = transport.posts.lock().clone();
    let mut failures = compare(&posts, &recorded, name);
    if totals.conversation_id != "c-1" {
        failures.push(format!(
            "{name}: conversation_id {}",
            totals.conversation_id
        ));
    }
    let total_messages = posts
        .iter()
        .filter_map(|body| {
            serde_json::from_str::<testkit::Json>(body)
                .ok()
                .and_then(|value| {
                    value
                        .get("messages")
                        .and_then(testkit::Json::as_array)
                        .map(Vec::len)
                })
        })
        .sum::<usize>();
    if totals.inserted != u64::try_from(total_messages).unwrap_or(u64::MAX) || totals.skipped != 0 {
        failures.push(format!(
            "{name}: totals inserted {} skipped {}",
            totals.inserted, totals.skipped
        ));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} writer failure(s):\n{}",
            failures.len(),
            failures.join("\n---\n")
        )
        .into())
    }
}

#[test]
fn writer_replay_cases() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(run_case("single", 768 * 1024, None))?;
    runtime.block_on(run_case("numbers_and_keys", 768 * 1024, None))?;
    runtime.block_on(run_case("surrogate_cap", 768 * 1024, None))?;
    runtime.block_on(run_case("finalize_multi_chunk", 24576, None))?;
    runtime.block_on(run_case("elide_tool_args", 32768, None))?;
    let env = config::Environment::default();
    let settings = config::RedactionSettings {
        enabled: true,
        builtins: true,
        patterns: vec!["SECRET[0-9]+".to_owned()],
    };
    let redaction =
        capture::redaction::resolve(Some(settings), &env).ok_or("redaction should resolve")?;
    runtime.block_on(run_case("redaction", 768 * 1024, Some(redaction)))
}
