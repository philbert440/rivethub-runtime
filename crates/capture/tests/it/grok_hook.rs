use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use parking_lot::Mutex;
use testkit::Json;
use wire::capture::CaptureResult;

use capture::hook::grok::GrokHook;
use capture::listing::store_roots;
use capture::spool::CaptureSpool;
use capture::writer::CaptureTransport;
use capture::writer::CaptureWriter;
use capture::writer::PostError;

const SESSION: &str = "33333333-3333-3333-3333-333333333333";
const NOW_TEXT: &str = "2026-10-11T12:05:00.000Z";

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Clone, Default)]
struct FakeTransport {
    posts: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl CaptureTransport for FakeTransport {
    async fn post(&self, body: String) -> Result<CaptureResult, PostError> {
        self.posts.lock().push(body);
        Ok(CaptureResult {
            ok: true,
            conversation_id: "conv-1".to_owned(),
            inserted: 1,
            skipped: 0,
        })
    }
}

fn hook(
    home: &Path,
    transport: &FakeTransport,
) -> Result<GrokHook<FakeTransport>, Box<dyn std::error::Error>> {
    let session = home.join(".grok/sessions/%2Fwork%2Fdemo").join(SESSION);
    std::fs::create_dir_all(&session)?;
    std::fs::copy(
        testkit::recording("capture/stores/grok-updates-open.jsonl"),
        session.join("updates.jsonl"),
    )?;
    std::fs::write(
        session.join("summary.json"),
        format!(
            r#"{{"info":{{"id":"{SESSION}","cwd":"/work/demo"}},"generated_title":"List the demo files","updated_at":"2026-10-11T12:00:30Z"}}"#
        ),
    )?;
    let env = config::Environment::from_vars([("HOME".to_owned(), home.display().to_string())]);
    let writer = CaptureWriter::new(
        transport.clone(),
        CaptureSpool::new(home.join("spool")),
        None,
        None,
    );
    GrokHook::new(writer, &store_roots(&env), &config::Paths::new(&env)?)
        .ok_or_else(|| "grok home".into())
}

fn at<'a>(batch: &'a Json, pointer: &str) -> &'a Json {
    batch.pointer(pointer).unwrap_or(&Json::Null)
}

fn payload(event: &str) -> String {
    format!(
        r#"{{"hookEventName":"stop","hook_event_name":"{event}","sessionId":"{SESSION}","session_id":"{SESSION}","cwd":"/work/demo"}}"#
    )
}

#[tokio::test]
async fn a_turn_end_posts_the_whole_log_as_a_titled_grok_conversation() -> TestResult {
    let home = tempfile::tempdir()?;
    let transport = FakeTransport::default();
    let hook = hook(home.path(), &transport)?;
    let now: DateTime<Utc> = NOW_TEXT.parse()?;
    let run = hook.run(&payload("Stop"), now).await;
    assert_eq!(run.stderr, "");
    let posts = transport.posts.lock().clone();
    let [body] = posts.as_slice() else {
        return Err(format!("expected one post, got {}", posts.len()).into());
    };
    let batch: Json = serde_json::from_str(body)?;
    assert_eq!(
        at(&batch, "/session_key").as_str(),
        Some(format!("grok-build:{SESSION}").as_str())
    );
    assert_eq!(at(&batch, "/agent"), "rivet-grok");
    assert_eq!(at(&batch, "/channel"), "grok-build");
    assert_eq!(at(&batch, "/title"), "List the demo files");
    assert_eq!(at(&batch, "/settings/cwd"), "/work/demo");
    assert_eq!(at(&batch, "/settings/last_event"), "Stop");
    assert!(at(&batch, "/finalize").is_null());
    let ids: Vec<&str> = at(&batch, "/messages")
        .as_array()
        .ok_or("messages")?
        .iter()
        .filter_map(|message| at(message, "/event_id").as_str())
        .collect();
    assert_eq!(ids.len(), 15);
    assert_eq!(
        ids.first(),
        Some(&format!("grok-build:{SESSION}:0").as_str())
    );
    assert_eq!(
        ids.last(),
        Some(&format!("grok-build:{SESSION}:1010029").as_str())
    );
    Ok(())
}

#[tokio::test]
async fn only_turn_ends_compaction_and_session_end_capture() -> TestResult {
    let home = tempfile::tempdir()?;
    let transport = FakeTransport::default();
    let hook = hook(home.path(), &transport)?;
    let now: DateTime<Utc> = NOW_TEXT.parse()?;
    for event in ["SessionStart", "UserPromptSubmit", "PostToolUse"] {
        assert_eq!(hook.run(&payload(event), now).await.stderr, "");
    }
    assert!(transport.posts.lock().is_empty());
    assert_eq!(hook.run(&payload("PreCompact"), now).await.stderr, "");
    assert_eq!(hook.run(&payload("SessionEnd"), now).await.stderr, "");
    let posts = transport.posts.lock().clone();
    let finals: Vec<Json> = posts
        .iter()
        .map(|body| serde_json::from_str::<Json>(body).map(|batch| at(&batch, "/finalize").clone()))
        .collect::<Result<_, _>>()?;
    assert_eq!(finals, [Json::Null, Json::Bool(true)]);
    Ok(())
}

#[tokio::test]
async fn a_session_with_no_turns_yet_posts_nothing() -> TestResult {
    let home = tempfile::tempdir()?;
    let transport = FakeTransport::default();
    let hook = hook(home.path(), &transport)?;
    let session = home
        .path()
        .join(".grok/sessions/%2Fwork%2Fdemo")
        .join(SESSION);
    std::fs::write(
        session.join("updates.jsonl"),
        "{\"method\":\"_x.ai/session/update\",\"params\":{\"sessionId\":\"s\",\"update\":{\"sessionUpdate\":\"hook_execution\",\"event_name\":\"session_start\",\"runs\":[]}}}\n",
    )?;
    let now: DateTime<Utc> = NOW_TEXT.parse()?;
    assert_eq!(hook.run(&payload("SessionEnd"), now).await.stderr, "");
    assert!(transport.posts.lock().is_empty());
    Ok(())
}
