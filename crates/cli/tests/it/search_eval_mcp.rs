use std::path::Path;
use std::process::Command;
use std::process::Output;

use kernel::memory::SummaryKind;
use serde::Serialize;
use wire::message::Role;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const EVAL: &str = env!("CARGO_BIN_EXE_search-eval");
const SERVER: &str = env!("CARGO_BIN_EXE_search-eval-server");
const ROTATE: &str = "00000000-0000-4000-8000-000000000102";
const MIGRATION: &str = "00000000-0000-4000-8000-000000000103";
const ROTATION_SUMMARY: &str = "00000000-0000-4000-8000-400000000101";
const CUT_NEEDLE: &str = "finished cleanly";

#[derive(Serialize)]
struct Corpus {
    conversations: Vec<Conversation>,
    messages: Vec<Message>,
    summaries: Vec<Summary>,
}

#[derive(Serialize)]
struct Conversation {
    id: &'static str,
    agent: &'static str,
    created_at: &'static str,
}

#[derive(Serialize)]
struct Message {
    id: &'static str,
    conversation: &'static str,
    role: Role,
    content: String,
    created_at: &'static str,
}

#[derive(Serialize)]
struct Summary {
    id: &'static str,
    conversation: Option<&'static str>,
    content: &'static str,
    kind: SummaryKind,
    created_at: &'static str,
}

#[derive(Serialize)]
struct QuerySet {
    clock: &'static str,
    limit: u32,
    queries: Vec<EvalQuery>,
}

#[derive(Serialize)]
struct EvalQuery {
    id: &'static str,
    query: &'static str,
    relevant: Vec<&'static str>,
    must_contain: Vec<&'static str>,
}

fn long_migration_log() -> String {
    format!(
        "the zebra migration log follows: {} and the run {CUT_NEEDLE}",
        "step ok; ".repeat(60)
    )
}

fn ops_corpus() -> Corpus {
    Corpus {
        conversations: vec![Conversation {
            id: "ops",
            agent: "rivet",
            created_at: "2026-10-01T09:00:00.000Z",
        }],
        messages: vec![
            Message {
                id: "00000000-0000-4000-8000-000000000101",
                conversation: "ops",
                role: Role::User,
                content: "where does the nightly backup land".to_owned(),
                created_at: "2026-10-01T09:01:00.000Z",
            },
            Message {
                id: ROTATE,
                conversation: "ops",
                role: Role::Assistant,
                content: "rotate it with the keyring tool, then restart the signer".to_owned(),
                created_at: "2026-10-01T09:02:00.000Z",
            },
            Message {
                id: MIGRATION,
                conversation: "ops",
                role: Role::Assistant,
                content: long_migration_log(),
                created_at: "2026-10-01T09:03:00.000Z",
            },
        ],
        summaries: vec![Summary {
            id: ROTATION_SUMMARY,
            conversation: Some("ops"),
            content: "The signer rotates through the keyring tool.",
            kind: SummaryKind::Leaf,
            created_at: "2026-10-01T09:10:00.000Z",
        }],
    }
}

fn keyring(limit: u32) -> QuerySet {
    QuerySet {
        clock: "2026-10-10T12:00:00Z",
        limit,
        queries: vec![EvalQuery {
            id: "keyring",
            query: "keyring tool",
            relevant: vec![ROTATE, ROTATION_SUMMARY],
            must_contain: vec!["keyring tool"],
        }],
    }
}

fn migration() -> QuerySet {
    QuerySet {
        clock: "2026-10-10T12:00:00Z",
        limit: 10,
        queries: vec![EvalQuery {
            id: "migration",
            query: "zebra migration",
            relevant: vec![MIGRATION],
            must_contain: vec![CUT_NEEDLE],
        }],
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> TestResult {
    std::fs::write(path, serde_json::to_string_pretty(value)?)?;
    Ok(())
}

fn run(args: &[&std::ffi::OsStr]) -> TestResult<(Output, String)> {
    let output = Command::new(EVAL).args(args).output()?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok((output, text))
}

fn seeded(dir: &Path, set: &QuerySet) -> TestResult<[std::path::PathBuf; 2]> {
    let corpus = dir.join("corpus.json");
    write_json(&corpus, &ops_corpus())?;
    let queries = dir.join("queries.json");
    write_json(&queries, set)?;
    Ok([corpus, queries])
}

#[test]
fn the_served_tool_meets_the_baseline_saved_in_process() -> TestResult {
    let dir = tempfile::tempdir()?;
    let [corpus, queries] = seeded(dir.path(), &keyring(10))?;
    let baseline = dir.path().join("baseline.json");
    let (saved, text) = run(&[
        "--corpus".as_ref(),
        corpus.as_os_str(),
        "--queries".as_ref(),
        queries.as_os_str(),
        "--baseline".as_ref(),
        baseline.as_os_str(),
        "--save-baseline".as_ref(),
    ])?;
    assert_eq!(saved.status.code(), Some(0), "{text}");
    let (checked, text) = run(&[
        "--corpus".as_ref(),
        corpus.as_os_str(),
        "--queries".as_ref(),
        queries.as_os_str(),
        "--baseline".as_ref(),
        baseline.as_os_str(),
        "--check".as_ref(),
        "--".as_ref(),
        SERVER.as_ref(),
        "{sqlite}".as_ref(),
    ])?;
    assert_eq!(checked.status.code(), Some(0), "{text}");
    assert!(
        text.contains("relevant: 2/2 found, first at rank 1\n  must_contain: pass"),
        "{text}"
    );
    assert!(text.contains("recall@10=1.000  mrr=1.000"), "{text}");
    assert!(text.contains("all metrics within tolerance"), "{text}");
    Ok(())
}

#[test]
fn a_hit_shown_cut_is_found_but_a_needle_past_the_cut_is_not() -> TestResult {
    let dir = tempfile::tempdir()?;
    let [corpus, queries] = seeded(dir.path(), &migration())?;
    let args = [
        "--corpus".as_ref(),
        corpus.as_os_str(),
        "--queries".as_ref(),
        queries.as_os_str(),
    ];
    let (in_process, text) = run(&args)?;
    assert_eq!(in_process.status.code(), Some(0), "{text}");
    assert!(text.contains("must_contain: pass"), "{text}");
    let served: Vec<&std::ffi::OsStr> = args
        .iter()
        .copied()
        .chain(["--".as_ref(), SERVER.as_ref(), "{sqlite}".as_ref()])
        .collect();
    let (output, text) = run(&served)?;
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("relevant: 1/1 found, first at rank 1\n  must_contain: FAIL"),
        "{text}"
    );
    Ok(())
}

const OWN_ROWS: &str = "INSERT INTO ros_conversations (id, session_key, agent, channel, created_at, updated_at) \
     VALUES ('own', 'own', 'rivet', 'cli', '2026-10-01T09:00:00.000Z', '2026-10-01T09:00:00.000Z'); \
     INSERT INTO ros_messages (id, conversation_id, role, content, agent, channel, created_at) \
     VALUES ('00000000-0000-4000-8000-000000000102', 'own', 'assistant', \
     'rotate it with the keyring tool, then restart the signer', 'rivet', 'cli', '2026-10-01T09:02:00.000Z');";

fn own_database(dir: &Path) -> TestResult<std::path::PathBuf> {
    let database = dir.join("own.sqlite");
    migrate::sqlite(&database)?;
    rusqlite::Connection::open(&database)?.execute_batch(OWN_ROWS)?;
    Ok(database)
}

#[test]
fn without_a_store_only_ids_in_the_listing_identify_hits() -> TestResult {
    let dir = tempfile::tempdir()?;
    let database = own_database(dir.path())?;
    let queries = dir.path().join("queries.json");
    write_json(&queries, &keyring(10))?;
    let (output, text) = run(&[
        "--queries".as_ref(),
        queries.as_os_str(),
        "--".as_ref(),
        SERVER.as_ref(),
        database.as_os_str(),
    ])?;
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains("relevant: 0/2 found, none in the results\n  must_contain: pass"),
        "{text}"
    );
    Ok(())
}

#[test]
fn a_pinned_database_reaches_the_server_as_a_copy() -> TestResult {
    let dir = tempfile::tempdir()?;
    let database = own_database(dir.path())?;
    let queries = dir.path().join("queries.json");
    write_json(&queries, &keyring(10))?;
    let before = std::fs::read(&database)?;
    let (output, text) = run(&[
        "--sqlite".as_ref(),
        database.as_os_str(),
        "--queries".as_ref(),
        queries.as_os_str(),
        "--".as_ref(),
        SERVER.as_ref(),
        "{sqlite}".as_ref(),
    ])?;
    assert_eq!(output.status.code(), Some(0), "{text}");
    assert!(
        text.contains("relevant: 1/2 found, first at rank 1\n  must_contain: pass"),
        "{text}"
    );
    assert_eq!(std::fs::read(&database)?, before);
    let mut entries: Vec<_> = std::fs::read_dir(dir.path())?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    assert_eq!(entries, ["own.sqlite", "queries.json"]);
    Ok(())
}

#[test]
fn a_tool_error_is_an_error_not_a_verdict() -> TestResult {
    let dir = tempfile::tempdir()?;
    let [corpus, queries] = seeded(dir.path(), &keyring(60))?;
    let (output, _) = run(&[
        "--corpus".as_ref(),
        corpus.as_os_str(),
        "--queries".as_ref(),
        queries.as_os_str(),
        "--".as_ref(),
        SERVER.as_ref(),
        "{sqlite}".as_ref(),
    ])?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.starts_with(
            "search-eval: query keyring: memory_search answered with an error: Input validation error"
        ),
        "{stderr}"
    );
    Ok(())
}

#[test]
fn a_server_that_cannot_start_is_an_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let [corpus, queries] = seeded(dir.path(), &keyring(10))?;
    let missing = dir.path().join("absent-server");
    let (output, _) = run(&[
        "--corpus".as_ref(),
        corpus.as_os_str(),
        "--queries".as_ref(),
        queries.as_os_str(),
        "--".as_ref(),
        missing.as_os_str(),
    ])?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.starts_with("search-eval: cannot start the MCP server: "),
        "{stderr}"
    );
    Ok(())
}
