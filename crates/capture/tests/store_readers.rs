use std::path::Path;

use chrono::DateTime;
use chrono::Utc;
use kernel::transcript::TranscriptReader;
use kernel::transcript::TranscriptReaderError;
use kernel::transcript::Turn;
use wire::harness::HarnessId;
use wire::message::Role;
use wire::session_id::SessionId;

use capture::readers::OUT_OF_SLICE;
use capture::readers::SkipRule;
use capture::readers::StoreReader;
use capture::readers::TranscriptSlice;

const SESSION: &str = "33333333-3333-3333-3333-333333333333";

const HERMES_SCHEMA: &str = "\
CREATE TABLE messages (\
    session_id TEXT NOT NULL,\
    role TEXT NOT NULL,\
    content TEXT NOT NULL,\
    timestamp TEXT NOT NULL\
);";

const OPENCODE_SCHEMA: &str = "\
CREATE TABLE message (\
    id TEXT PRIMARY KEY,\
    session_id TEXT NOT NULL,\
    role TEXT NOT NULL,\
    created_at TEXT NOT NULL\
);\
CREATE TABLE part (\
    id TEXT PRIMARY KEY,\
    message_id TEXT NOT NULL,\
    session_id TEXT NOT NULL,\
    type TEXT NOT NULL,\
    text TEXT,\
    tool_name TEXT,\
    tool_arguments TEXT,\
    tool_result TEXT,\
    created_at TEXT NOT NULL\
);";

fn open(harness: HarnessId, root: &Path) -> Result<StoreReader, Box<dyn std::error::Error>> {
    StoreReader::open(harness, root.to_path_buf()).ok_or_else(|| "reader".into())
}

fn session(harness: HarnessId) -> SessionId {
    SessionId::new(harness, SESSION)
}

fn place(root: &Path, relative: &str, fixture: &str) -> Result<(), Box<dyn std::error::Error>> {
    let destination = root.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(testkit::recording(fixture), &destination)?;
    Ok(())
}

fn stamp(text: &str) -> Result<DateTime<Utc>, chrono::ParseError> {
    text.parse()
}

fn dialogue(turns: &[Turn]) -> Result<(), Box<dyn std::error::Error>> {
    let expected = [
        (Role::User, "list the files", "2026-10-14T12:00:00Z"),
        (Role::Assistant, "Running ls.", "2026-10-14T12:00:01Z"),
        (Role::Tool, "a.txt", "2026-10-14T12:00:02Z"),
    ];
    if turns.len() != expected.len() {
        return Err(format!("{} turns", turns.len()).into());
    }
    for (turn, (role, content, at)) in turns.iter().zip(expected) {
        if turn.role != role || turn.content != content || turn.at != stamp(at)? {
            return Err(format!("{} {}", turn.content, turn.at).into());
        }
    }
    Ok(())
}

fn only_unknown(slice: &TranscriptSlice) -> bool {
    slice.skipped.len() == 1
        && slice
            .skipped
            .first()
            .is_some_and(|record| record.rule == SkipRule::UnknownRecordType)
}

fn tool_named(slice: &TranscriptSlice, name: &str) -> bool {
    slice
        .messages
        .iter()
        .any(|message| message.role == Role::Tool && message.tool_name.as_deref() == Some(name))
}

async fn replay(
    reader: &StoreReader,
    harness: HarnessId,
    root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let session = session(harness);
    if reader.sessions(root).await? != vec![session.clone()] {
        return Err("session list".into());
    }
    dialogue(&reader.read(&session).await?)?;
    let tail = reader
        .tail(&session, stamp("2026-10-14T12:00:01Z")?)
        .await?;
    if tail.len() != 1 || tail.first().map(|turn| turn.content.as_str()) != Some("a.txt") {
        return Err("tail".into());
    }
    let slice = reader.messages(&session, 0).await?;
    if !only_unknown(&slice) || !tool_named(&slice, "bash") {
        return Err("messages".into());
    }
    let again = reader.messages(&session, slice.next_offset).await?;
    if !again.messages.is_empty() || !again.skipped.is_empty() {
        return Err("cursor".into());
    }
    Ok(())
}

fn codex_name() -> String {
    format!("sessions/2026/10/14/rollout-2026-10-14T12-00-00-{SESSION}.jsonl")
}

#[tokio::test]
async fn codex_rollout_maps_events_and_paired_tools() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    place(
        dir.path(),
        &codex_name(),
        "capture/stores/codex-rollout.jsonl",
    )?;
    let reader = open(HarnessId::Codex, dir.path())?;
    let session = session(HarnessId::Codex);
    let slice = reader.messages(&session, 0).await?;
    let rules: Vec<SkipRule> = slice.skipped.iter().map(|record| record.rule).collect();
    if rules != [SkipRule::UnknownRecordType, SkipRule::UnparseableRecord] {
        return Err(format!("{rules:?}").into());
    }
    dialogue(&reader.read(&session).await?)?;
    if !tool_named(&slice, "bash") {
        return Err("tool name".into());
    }
    let resumed = reader.messages(&session, 5).await?;
    if resumed.messages.len() != 1 || !tool_named(&resumed, "bash") {
        return Err("paired call before the cursor".into());
    }
    let caught_up = reader.messages(&session, slice.next_offset).await?;
    if !caught_up.messages.is_empty() {
        return Err("caught up".into());
    }
    let path = dir.path().join(codex_name());
    let mut appended = std::fs::read_to_string(&path)?;
    appended.push_str(
        "{\"timestamp\":\"2026-10-14T12:00:04Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"more\"}}\n",
    );
    std::fs::write(&path, appended)?;
    let fresh = reader.messages(&session, slice.next_offset).await?;
    if fresh.messages.len() != 1
        || fresh
            .messages
            .first()
            .map(|message| message.content.as_str())
            != Some("more")
    {
        return Err("append".into());
    }
    Ok(())
}

#[tokio::test]
async fn grok_chat_history_keeps_parent_links_out_of_the_turns()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let folder = format!("sessions/-work-demo/{SESSION}");
    place(
        dir.path(),
        &format!("{folder}/chat_history.jsonl"),
        "capture/stores/grok-chat-history.jsonl",
    )?;
    place(
        dir.path(),
        &format!("{folder}/summary.json"),
        "capture/stores/grok-summary.json",
    )?;
    place(
        dir.path(),
        &format!("{folder}/updates.jsonl"),
        "capture/stores/grok-updates.jsonl",
    )?;
    replay(
        &open(HarnessId::GrokBuild, dir.path())?,
        HarnessId::GrokBuild,
        dir.path(),
    )
    .await
}

#[tokio::test]
async fn kimi_wire_pairs_tool_calls_and_ignores_the_index() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tempfile::tempdir()?;
    let folder = format!("sessions/wd_demo_hash/session_{SESSION}");
    place(
        dir.path(),
        &format!("{folder}/agents/main/wire.jsonl"),
        "capture/stores/kimi-wire.jsonl",
    )?;
    place(
        dir.path(),
        &format!("{folder}/session_index.jsonl"),
        "capture/stores/kimi-index.jsonl",
    )?;
    replay(
        &open(HarnessId::KimiCode, dir.path())?,
        HarnessId::KimiCode,
        dir.path(),
    )
    .await
}

#[tokio::test]
async fn pi_jsonl_skips_the_session_header() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    place(
        dir.path(),
        &format!("agent/sessions/---work-demo--/2026-10-14T12-00-00_{SESSION}.jsonl"),
        "capture/stores/pi.jsonl",
    )?;
    replay(&open(HarnessId::Pi, dir.path())?, HarnessId::Pi, dir.path()).await
}

#[tokio::test]
async fn qwen_jsonl_skips_the_runtime_sidecar() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let chats = format!("projects/-work-demo/chats/{SESSION}.jsonl");
    place(dir.path(), &chats, "capture/stores/qwen.jsonl")?;
    let sidecar = dir
        .path()
        .join(format!("projects/-work-demo/chats/{SESSION}.runtime.json"));
    std::fs::write(&sidecar, "{}")?;
    replay(
        &open(HarnessId::QwenCode, dir.path())?,
        HarnessId::QwenCode,
        dir.path(),
    )
    .await
}

#[tokio::test]
async fn cursor_jsonl_reads_role_tags() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    place(
        dir.path(),
        &format!("projects/work-demo/agent-transcripts/{SESSION}/{SESSION}.jsonl"),
        "capture/stores/cursor.jsonl",
    )?;
    replay(
        &open(HarnessId::Cursor, dir.path())?,
        HarnessId::Cursor,
        dir.path(),
    )
    .await
}

fn hermes_store(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let connection = rusqlite::Connection::open(dir.join("state.db"))?;
    connection.execute_batch(HERMES_SCHEMA)?;
    let insert =
        "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?1, ?2, ?3, ?4)";
    connection.execute(
        insert,
        (SESSION, "assistant", "Running ls.", "2026-10-14T12:00:01Z"),
    )?;
    connection.execute(
        insert,
        (SESSION, "user", "list the files", "2026-10-14T12:00:00Z"),
    )?;
    connection.execute(insert, (SESSION, "tool", "a.txt", "2026-10-14T12:00:02Z"))?;
    connection.execute(
        insert,
        (SESSION, "narrator", "aside", "2026-10-14T12:00:03Z"),
    )?;
    connection.execute(
        insert,
        ("other", "user", "elsewhere", "2026-10-14T12:00:00Z"),
    )?;
    Ok(())
}

#[tokio::test]
async fn hermes_messages_map_roles_and_report_an_unknown_speaker()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    hermes_store(dir.path())?;
    let reader = StoreReader::open(HarnessId::Hermes, dir.path().to_path_buf()).expect("reader");
    let session = session(HarnessId::Hermes);
    let listed = reader.sessions(dir.path()).await?;
    if listed != [session.clone(), SessionId::new(HarnessId::Hermes, "other")] {
        return Err(format!("sessions {}", listed.len()).into());
    }
    dialogue(&reader.read(&session).await?)?;
    let slice = reader.messages(&session, 0).await?;
    if slice
        .messages
        .iter()
        .any(|message| message.tool_name.is_some())
    {
        return Err("hermes rows have no tool name".into());
    }
    let speaker = slice.skipped.first();
    if slice.skipped.len() != 1
        || speaker.is_none_or(|record| record.rule != SkipRule::UnrecognizedSpeaker)
    {
        return Err("speaker".into());
    }
    let again = reader.messages(&session, 3).await?;
    if !again.messages.is_empty() || again.skipped.len() != 1 {
        return Err("row cursor".into());
    }
    Ok(())
}

struct PartRow {
    id: &'static str,
    message: &'static str,
    kind: &'static str,
    text: Option<&'static str>,
    name: Option<&'static str>,
    arguments: Option<&'static str>,
    result: Option<&'static str>,
    at: &'static str,
}

fn opencode_store(dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let connection = rusqlite::Connection::open(dir.join("opencode.db"))?;
    connection.execute_batch(OPENCODE_SCHEMA)?;
    let message = "INSERT INTO message (id, session_id, role, created_at) VALUES (?1, ?2, ?3, ?4)";
    connection.execute(message, ("m1", SESSION, "user", "2026-10-14T12:00:00Z"))?;
    connection.execute(
        message,
        ("m2", SESSION, "assistant", "2026-10-14T12:00:01Z"),
    )?;
    insert_parts(&connection)?;
    Ok(())
}

fn insert_parts(connection: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    let part = "INSERT INTO part (id, message_id, session_id, type, text, tool_name, tool_arguments, tool_result, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";
    for row in part_rows() {
        connection.execute(
            part,
            (
                row.id,
                row.message,
                SESSION,
                row.kind,
                row.text,
                row.name,
                row.arguments,
                row.result,
                row.at,
            ),
        )?;
    }
    Ok(())
}

fn part_rows() -> [PartRow; 5] {
    [
        PartRow {
            id: "p3",
            message: "m2",
            kind: "tool",
            text: None,
            name: Some("bash"),
            arguments: Some(r#"{"cmd":"ls"}"#),
            result: Some("a.txt"),
            at: "2026-10-14T12:00:02Z",
        },
        PartRow {
            id: "p1",
            message: "m1",
            kind: "text",
            text: Some("list the files"),
            name: None,
            arguments: None,
            result: None,
            at: "2026-10-14T12:00:00Z",
        },
        PartRow {
            id: "p2",
            message: "m2",
            kind: "text",
            text: Some("Running ls."),
            name: None,
            arguments: None,
            result: None,
            at: "2026-10-14T12:00:01Z",
        },
        PartRow {
            id: "p4",
            message: "m2",
            kind: "reasoning",
            text: Some("aside"),
            name: None,
            arguments: None,
            result: None,
            at: "2026-10-14T12:00:03Z",
        },
        PartRow {
            id: "p5",
            message: "missing",
            kind: "text",
            text: Some("orphan"),
            name: None,
            arguments: None,
            result: None,
            at: "2026-10-14T12:00:04Z",
        },
    ]
}

#[tokio::test]
async fn opencode_parts_map_text_tools_and_orphans() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    opencode_store(dir.path())?;
    let reader = StoreReader::open(HarnessId::Opencode, dir.path().to_path_buf()).expect("reader");
    let session = session(HarnessId::Opencode);
    dialogue(&reader.read(&session).await?)?;
    let slice = reader.messages(&session, 0).await?;
    if !tool_named(&slice, "bash") {
        return Err("tool".into());
    }
    let rules: Vec<SkipRule> = slice.skipped.iter().map(|record| record.rule).collect();
    if rules != [SkipRule::UnknownRecordType, SkipRule::UnrecognizedSpeaker] {
        return Err(format!("{rules:?}").into());
    }
    Ok(())
}

#[tokio::test]
async fn sqlite_open_failure_lists_no_sessions() -> Result<(), Box<dyn std::error::Error>> {
    let missing = tempfile::tempdir()?;
    let reader =
        StoreReader::open(HarnessId::Hermes, missing.path().to_path_buf()).expect("reader");
    if !reader.sessions(missing.path()).await?.is_empty() {
        return Err("missing database listed sessions".into());
    }
    let present = tempfile::tempdir()?;
    std::fs::write(present.path().join("state.db"), "not a database")?;
    let reader =
        StoreReader::open(HarnessId::Hermes, present.path().to_path_buf()).expect("reader");
    if !reader.sessions(present.path()).await?.is_empty() {
        return Err("unreadable database listed sessions".into());
    }
    let opencode = tempfile::tempdir()?;
    std::fs::write(opencode.path().join("opencode.db"), "not a database")?;
    let reader =
        StoreReader::open(HarnessId::Opencode, opencode.path().to_path_buf()).expect("reader");
    if !reader.sessions(opencode.path()).await?.is_empty() {
        return Err("unreadable opencode database listed sessions".into());
    }
    Ok(())
}

#[tokio::test]
async fn sqlite_query_failure_is_a_read_error() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let connection = rusqlite::Connection::open(dir.path().join("opencode.db"))?;
    connection.execute_batch("CREATE TABLE other (id TEXT);")?;
    drop(connection);
    let reader = StoreReader::open(HarnessId::Opencode, dir.path().to_path_buf()).expect("reader");
    let error = reader.sessions(dir.path()).await.unwrap_err();
    if !matches!(error, TranscriptReaderError::Read { .. }) {
        return Err("query failure was not a read error".into());
    }
    let hermes = tempfile::tempdir()?;
    let connection = rusqlite::Connection::open(hermes.path().join("state.db"))?;
    connection.execute_batch("CREATE TABLE other (id TEXT);")?;
    drop(connection);
    let reader = StoreReader::open(HarnessId::Hermes, hermes.path().to_path_buf()).expect("reader");
    let error = reader.sessions(hermes.path()).await.unwrap_err();
    if !matches!(error, TranscriptReaderError::Read { .. }) {
        return Err("hermes query failure was not a read error".into());
    }
    Ok(())
}

#[tokio::test]
async fn out_of_slice_names_are_the_scenarios_this_slice_does_not_read() {
    let names: Vec<&str> = OUT_OF_SLICE.iter().map(|(name, _)| *name).collect();
    for (name, reason) in OUT_OF_SLICE {
        assert!(!reason.is_empty(), "{name}");
    }
    for harness in [
        "claude-code",
        "grok-build",
        "kimi-code",
        "hermes",
        "codex",
        "opencode",
        "pi",
        "qwen-code",
        "cursor",
    ] {
        assert!(!names.contains(&harness), "{harness}");
    }
    assert!(names.contains(&"cowork"));
    assert!(StoreReader::open(HarnessId::Cowork, std::path::PathBuf::new()).is_none());
    assert!(StoreReader::open(HarnessId::Codex, std::path::PathBuf::new()).is_some());
}

#[tokio::test]
async fn claude_messages_match_the_recorded_turns() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let project = dir.path().join("projects").join("-work-demo");
    std::fs::create_dir_all(&project)?;
    std::fs::copy(
        testkit::recording("capture/claude-hook/transcript.jsonl"),
        project.join(format!("{SESSION}.jsonl")),
    )?;
    let reader = open(HarnessId::ClaudeCode, dir.path())?;
    let session = SessionId::new(HarnessId::ClaudeCode, SESSION);
    let turns = reader.read(&session).await?;
    let slice = reader.messages(&session, 0).await?;
    if turns.len() != slice.messages.len() {
        return Err("message count".into());
    }
    for (turn, message) in turns.iter().zip(&slice.messages) {
        if turn.role != message.role || turn.content != message.content {
            return Err(message.content.clone().into());
        }
    }
    let text = "{\"type\":\"summary\",\"timestamp\":\"2026-10-10T12:00:00Z\"}\n";
    let parsed = capture::transcript::parse_lines(text);
    let turns = capture::transcript::turns_of(&parsed.lines);
    if turns.turns.is_empty()
        && turns
            .skipped
            .first()
            .is_some_and(|line| line.reason == "unknown record type")
    {
        Ok(())
    } else {
        Err("unknown claude record".into())
    }
}
