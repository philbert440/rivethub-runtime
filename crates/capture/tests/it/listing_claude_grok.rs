use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::Duration;
use std::time::SystemTime;

use capture::listing::StoreRoots;
use capture::listing::StoreSessionLister;
use kernel::transcript::ListLimit;
use kernel::transcript::SessionLister;
use kernel::transcript::StoreSession;
use wire::harness::HarnessId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const U1: &str = "00000000-0000-4000-8000-000000000001";
const P1: &str = "00000000-0000-4000-8000-0000000000a1";
const C1: &str = "00000000-0000-4000-8000-0000000000c1";

fn write(root: &Path, relative: &str, content: &str) -> Result<std::path::PathBuf, std::io::Error> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, content)?;
    Ok(path)
}

fn set_mtime(path: &Path, ms: u64) -> Result<(), std::io::Error> {
    fs::File::options()
        .write(true)
        .open(path)?
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_millis(ms))
}

fn claude_lister(root: &Path) -> StoreSessionLister {
    StoreSessionLister::new(StoreRoots {
        claude: Some(root.to_path_buf()),
        ..StoreRoots::default()
    })
}

fn grok_lister(root: &Path) -> StoreSessionLister {
    StoreSessionLister::new(StoreRoots {
        grok: Some(root.to_path_buf()),
        ..StoreRoots::default()
    })
}

async fn claude_rows(root: &Path) -> Vec<StoreSession> {
    claude_lister(root)
        .list(&[HarnessId::ClaudeCode], ListLimit::DEFAULT)
        .await
}

async fn claude_title(lines: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        &format!("projects/-home-a/{U1}.jsonl"),
        &lines.join("\n"),
    )?;
    let rows = claude_rows(dir.path()).await;
    let [row] = rows.as_slice() else {
        return Err(format!("{} rows", rows.len()).into());
    };
    Ok(row.title.clone())
}

fn user(content: &str) -> String {
    format!(r#"{{"type":"user","message":{{"content":{content}}}}}"#)
}

#[tokio::test]
async fn claude_summary_line_titles_the_session() -> TestResult {
    let dir = tempfile::tempdir()?;
    let lines = [
        r#"{"type":"summary","summary":"  Fix WS  "}"#.to_owned(),
        user(r#""ignored""#),
    ];
    write(
        dir.path(),
        &format!("projects/-home-a/{U1}.jsonl"),
        &lines.join("\n"),
    )?;
    let rows = claude_rows(dir.path()).await;
    let row = rows.first().ok_or("no row")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(row.id, U1);
    assert_eq!(row.harness, HarnessId::ClaudeCode);
    assert_eq!(row.title, "Fix WS");
    assert!(row.created_at.is_some());
    assert_eq!(row.parent_session_id, None);
    Ok(())
}

#[tokio::test]
async fn claude_title_skips_tool_results() -> TestResult {
    let title = claude_title(&[
        &user(r#"[{"type":"tool_result","content":"x"}]"#),
        &user(r#""hello""#),
    ])
    .await?;
    assert_eq!(title, "hello");
    Ok(())
}

#[tokio::test]
async fn claude_title_keeps_command_wrappers() -> TestResult {
    let title = claude_title(&[&user(r#""<command-name>/clear</command-name>""#)]).await?;
    assert_eq!(title, "<command-name>/clear</command-name>");
    Ok(())
}

#[tokio::test]
async fn claude_title_concatenates_text_blocks_without_separator() -> TestResult {
    let title = claude_title(&[&user(
        r#"[{"type":"text","text":"a"},{"type":"text","text":"b"}]"#,
    )])
    .await?;
    assert_eq!(title, "ab");
    Ok(())
}

#[tokio::test]
async fn claude_title_unwraps_matching_pasted_content() -> TestResult {
    let matched = claude_title(&[&user(
        r#""<pasted_content id=\"p1\">\nhello\n</pasted_content id=\"p1\">""#,
    )])
    .await?;
    assert_eq!(matched, "hello");
    let bare_close = claude_title(&[&user(
        r#""see <pasted_content id=\"p1\">body</pasted_content> end""#,
    )])
    .await?;
    assert_eq!(bare_close, "see body end");
    let mismatched = claude_title(&[&user(
        r#""<pasted_content id=\"p1\">\nhello\n</pasted_content id=\"p2\">""#,
    )])
    .await?;
    assert_eq!(
        mismatched,
        "<pasted_content id=\"p1\">\nhello\n</pasted_content id=\"p2\">"
    );
    Ok(())
}

#[tokio::test]
async fn claude_title_clips_to_120_chars() -> TestResult {
    let long = "x".repeat(300);
    let title = claude_title(&[&user(&format!("\"{long}\""))]).await?;
    assert_eq!(title, "x".repeat(120));
    let accented = "é".repeat(130);
    let title = claude_title(&[&user(&format!("\"{accented}\""))]).await?;
    assert_eq!(title.chars().count(), 120);
    assert_eq!(title, "é".repeat(120));
    Ok(())
}

#[tokio::test]
async fn claude_title_reads_only_the_first_64_kib() -> TestResult {
    let padding = format!(r#"{{"type":"progress","pad":"{}"}}"#, "p".repeat(70_000));
    let title = claude_title(&[&padding, &user(r#""late""#)]).await?;
    assert_eq!(title, U1);
    Ok(())
}

#[tokio::test]
async fn claude_lists_subagents_in_every_layout() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "projects/-home-a/S/subagents/agent-a1.jsonl",
        &user(r#""one""#),
    )?;
    write(
        dir.path(),
        "projects/-home-a/S/subagents/workflows/r1/agent-a2.jsonl",
        &user(r#""two""#),
    )?;
    write(
        dir.path(),
        "projects/-home-a/S/agent-a3.jsonl",
        &user(r#""three""#),
    )?;
    let mut rows = claude_rows(dir.path()).await;
    rows.sort_by(|left, right| left.id.cmp(&right.id));
    let ids: Vec<(&str, Option<&str>, &str)> = rows
        .iter()
        .map(|row| {
            (
                row.id.as_str(),
                row.parent_session_id.as_deref(),
                row.title.as_str(),
            )
        })
        .collect();
    assert_eq!(
        ids,
        vec![
            ("a1", Some("S"), "one"),
            ("a2", Some("S"), "two"),
            ("a3", Some("S"), "three")
        ]
    );
    Ok(())
}

#[tokio::test]
async fn claude_subagent_labels_come_from_the_meta_sidecar() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "projects/-home-a/S/subagents/agent-a1.jsonl",
        &user(r#""one""#),
    )?;
    write(
        dir.path(),
        "projects/-home-a/S/subagents/agent-a1.meta.json",
        r#"{"agentType":"explore","model":" m1 "}"#,
    )?;
    let rows = claude_rows(dir.path()).await;
    let row = rows.first().ok_or("no row")?;
    assert_eq!(row.agent_name.as_deref(), Some("explore"));
    assert_eq!(row.model.as_deref(), Some("m1"));
    Ok(())
}

#[tokio::test]
async fn claude_non_string_meta_label_blocks_fallthrough_then_head_scan_applies() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "projects/-home-a/S/subagents/agent-a1.jsonl",
        &[
            r#"{"type":"user","slug":"random-name","agentType":"from-head","message":{"model":"head-model","content":"go"}}"#,
        ]
        .join("\n"),
    )?;
    write(
        dir.path(),
        "projects/-home-a/S/subagents/agent-a1.meta.json",
        r#"{"agentType":7,"subagent_type":"x"}"#,
    )?;
    let rows = claude_rows(dir.path()).await;
    let row = rows.first().ok_or("no row")?;
    assert_eq!(row.agent_name.as_deref(), Some("from-head"));
    assert_eq!(row.model.as_deref(), Some("head-model"));
    Ok(())
}

#[tokio::test]
async fn claude_same_agent_under_two_parents_is_ambiguous_to_describe() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "projects/-home-a/S/subagents/agent-a1.jsonl",
        &user(r#""s""#),
    )?;
    write(
        dir.path(),
        "projects/-home-a/T/subagents/agent-a1.jsonl",
        &user(r#""t""#),
    )?;
    let rows = claude_rows(dir.path()).await;
    let mut parents: Vec<Option<&str>> = rows
        .iter()
        .map(|row| row.parent_session_id.as_deref())
        .collect();
    parents.sort_unstable();
    assert_eq!(parents, vec![Some("S"), Some("T")]);
    let lister = claude_lister(dir.path());
    assert_eq!(
        lister.describe(HarnessId::ClaudeCode, "a1", None).await?,
        None
    );
    let described = lister
        .describe(HarnessId::ClaudeCode, "a1", Some("T"))
        .await?
        .ok_or("parent T")?;
    assert_eq!(described.parent_session_id.as_deref(), Some("T"));
    assert_eq!(described.title, "t");
    Ok(())
}

#[tokio::test]
async fn claude_drops_subagent_files_shadowed_by_a_top_level_session() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(dir.path(), "projects/-home-a/X.jsonl", &user(r#""top""#))?;
    write(
        dir.path(),
        "projects/-home-a/S/agent-X.jsonl",
        &user(r#""nested""#),
    )?;
    let rows = claude_rows(dir.path()).await;
    let ids: Vec<(&str, Option<&str>)> = rows
        .iter()
        .map(|row| (row.id.as_str(), row.parent_session_id.as_deref()))
        .collect();
    assert_eq!(ids, vec![("X", None)]);
    Ok(())
}

#[tokio::test]
async fn claude_missing_projects_dir_is_an_empty_list() -> TestResult {
    let dir = tempfile::tempdir()?;
    assert_eq!(claude_rows(dir.path()).await, Vec::<StoreSession>::new());
    assert_eq!(
        claude_rows(&dir.path().join("absent")).await,
        Vec::<StoreSession>::new()
    );
    let lister = claude_lister(dir.path());
    assert_eq!(
        lister.describe(HarnessId::ClaudeCode, U1, None).await?,
        None
    );
    Ok(())
}

#[tokio::test]
async fn claude_ignores_names_containing_dot_dot() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(dir.path(), "projects/a..b/kept-out.jsonl", &user(r#""x""#))?;
    write(dir.path(), "projects/-home-a/x..y.jsonl", &user(r#""x""#))?;
    write(dir.path(), "projects/-home-a/ok.jsonl", &user(r#""x""#))?;
    let rows = claude_rows(dir.path()).await;
    let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
    assert_eq!(ids, vec!["ok"]);
    Ok(())
}

#[tokio::test]
async fn claude_ranks_by_mtime_and_keeps_the_newer_duplicate() -> TestResult {
    let dir = tempfile::tempdir()?;
    let old = write(
        dir.path(),
        "projects/-home-a/dup.jsonl",
        &user(r#""old copy""#),
    )?;
    let new = write(
        dir.path(),
        "projects/-home-b/dup.jsonl",
        &user(r#""new copy""#),
    )?;
    let other = write(
        dir.path(),
        "projects/-home-a/other.jsonl",
        &user(r#""other""#),
    )?;
    set_mtime(&old, 1_000_000)?;
    set_mtime(&new, 3_000_000)?;
    set_mtime(&other, 2_000_000)?;
    let rows = claude_rows(dir.path()).await;
    let ids: Vec<(&str, &str, i64)> = rows
        .iter()
        .map(|row| (row.id.as_str(), row.title.as_str(), row.updated_at))
        .collect();
    assert_eq!(
        ids,
        vec![
            ("dup", "new copy", 3_000_000),
            ("other", "other", 2_000_000)
        ]
    );
    Ok(())
}

#[tokio::test]
async fn claude_store_cwd_is_the_first_cwd_line_of_a_uuid_session() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        &format!("projects/-home-a/{U1}.jsonl"),
        &[
            r#"{"type":"summary","cwd":"   "}"#,
            r#"{"type":"user","cwd":" /home/user/w "}"#,
        ]
        .join("\n"),
    )?;
    write(
        dir.path(),
        "projects/-home-a/plain.jsonl",
        r#"{"cwd":"/home/user/p"}"#,
    )?;
    let lister = claude_lister(dir.path());
    assert_eq!(
        lister
            .store_cwd(HarnessId::ClaudeCode, U1)
            .await?
            .as_deref(),
        Some("/home/user/w")
    );
    assert_eq!(
        lister.store_cwd(HarnessId::ClaudeCode, "plain").await?,
        None
    );
    Ok(())
}

#[tokio::test]
async fn claude_describe_finds_top_level_and_refuses_unknown_uuids() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        &format!("projects/-home-a/{U1}.jsonl"),
        &user(r#""hi""#),
    )?;
    let lister = claude_lister(dir.path());
    let row = lister
        .describe(HarnessId::ClaudeCode, U1, None)
        .await?
        .ok_or("described")?;
    assert_eq!(row.title, "hi");
    assert_eq!(row.parent_session_id, None);
    assert_eq!(
        lister.describe(HarnessId::ClaudeCode, P1, None).await?,
        None
    );
    assert_eq!(
        lister.describe(HarnessId::ClaudeCode, "../x", None).await?,
        None
    );
    Ok(())
}

fn grok_summary(id: &str, extra: &str) -> String {
    format!(
        r#"{{"info":{{"id":"{id}"}},"session_summary":"s-{id}","updated_at":"2026-10-01T00:00:00Z"{extra}}}"#
    )
}

fn spawn_line(child: &str, parent: &str) -> String {
    format!(
        r#"{{"params":{{"update":{{"sessionUpdate":"subagent_spawned","child_session_id":"{child}","parent_session_id":"{parent}"}}}}}}"#
    )
}

async fn grok_rows(lister: &StoreSessionLister) -> Vec<StoreSession> {
    lister
        .list(&[HarnessId::GrokBuild], ListLimit::DEFAULT)
        .await
}

fn parent_of<'a>(rows: &'a [StoreSession], id: &str) -> Option<&'a str> {
    rows.iter()
        .find(|row| row.id == id)
        .and_then(|row| row.parent_session_id.as_deref())
}

#[tokio::test]
async fn grok_summary_maps_to_a_row() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "sessions/enc/U1/summary.json",
        r#"{"info":{"id":"U1"},"session_summary":"plan","updated_at":"2026-10-01T00:00:00Z","current_model_id":"grok-4"}"#,
    )?;
    write(dir.path(), "sessions/session_search.sqlite", "not a dir")?;
    let rows = grok_rows(&grok_lister(dir.path())).await;
    let mut expected = StoreSession::new(HarnessId::GrokBuild, "U1", 1_790_812_800_000);
    expected.title = "plan".to_owned();
    expected.model = Some("grok-4".to_owned());
    assert_eq!(rows, vec![expected]);
    Ok(())
}

#[tokio::test]
async fn grok_nested_rows_take_their_parent_from_spawn_lines() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        &format!("sessions/enc/{P1}/summary.json"),
        &grok_summary(P1, ""),
    )?;
    write(
        dir.path(),
        &format!("sessions/enc/{P1}/updates.jsonl"),
        &format!("{}\n", spawn_line(C1, P1)),
    )?;
    write(
        dir.path(),
        &format!("sessions/enc/{C1}/summary.json"),
        &grok_summary(C1, r#","session_kind":"subagent""#),
    )?;
    let rows = grok_rows(&grok_lister(dir.path())).await;
    assert_eq!(parent_of(&rows, C1), Some(P1));
    assert_eq!(parent_of(&rows, P1), None);
    write(
        dir.path(),
        &format!("sessions/enc/{C1}/summary.json"),
        &grok_summary(C1, r#","session_kind":"headless""#),
    )?;
    let rows = grok_rows(&grok_lister(dir.path())).await;
    assert_eq!(parent_of(&rows, C1), None);
    Ok(())
}

#[tokio::test]
async fn grok_spawn_line_counts_only_once_complete() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        &format!("sessions/enc/{C1}/summary.json"),
        &grok_summary(C1, r#","session_kind":"subagent_fork""#),
    )?;
    let updates = write(
        dir.path(),
        &format!("sessions/enc/{P1}/updates.jsonl"),
        &spawn_line(C1, P1),
    )?;
    let lister = grok_lister(dir.path());
    assert_eq!(parent_of(&grok_rows(&lister).await, C1), None);
    fs::OpenOptions::new()
        .append(true)
        .open(&updates)?
        .write_all(b"\n")?;
    assert_eq!(parent_of(&grok_rows(&lister).await, C1), Some(P1));
    let described = lister
        .describe(HarnessId::GrokBuild, C1, None)
        .await?
        .ok_or("described")?;
    assert_eq!(described.parent_session_id.as_deref(), Some(P1));
    assert_eq!(
        parent_of(&grok_rows(&grok_lister(dir.path())).await, C1),
        Some(P1)
    );
    Ok(())
}

#[tokio::test]
async fn grok_non_string_child_id_blocks_the_subagent_id_fallthrough() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        &format!("sessions/enc/{C1}/summary.json"),
        &grok_summary(C1, r#","session_kind":"subagent""#),
    )?;
    let line = format!(
        r#"{{"params":{{"update":{{"sessionUpdate":"subagent_spawned","child_session_id":5,"subagent_id":"{C1}","parent_session_id":"{P1}"}}}}}}"#
    );
    write(
        dir.path(),
        &format!("sessions/enc/{P1}/updates.jsonl"),
        &format!("{line}\n"),
    )?;
    assert_eq!(
        parent_of(&grok_rows(&grok_lister(dir.path())).await, C1),
        None
    );
    let null_child = format!(
        r#"{{"params":{{"update":{{"sessionUpdate":"subagent_spawned","child_session_id":null,"subagent_id":" {C1} ","parent_session_id":"{P1}"}}}}}}"#
    );
    write(
        dir.path(),
        &format!("sessions/enc/{P1}/updates.jsonl"),
        &format!("{null_child}\n"),
    )?;
    assert_eq!(
        parent_of(&grok_rows(&grok_lister(dir.path())).await, C1),
        Some(P1)
    );
    Ok(())
}

#[tokio::test]
async fn grok_missing_updated_at_is_zero() -> TestResult {
    let dir = tempfile::tempdir()?;
    write(
        dir.path(),
        "sessions/enc/d1/summary.json",
        r#"{"session_summary":"ask"}"#,
    )?;
    write(dir.path(), "sessions/enc/d2/summary.json", "not json")?;
    let rows = grok_rows(&grok_lister(dir.path())).await;
    assert_eq!(
        rows,
        vec![StoreSession {
            title: "ask".to_owned(),
            ..StoreSession::new(HarnessId::GrokBuild, "d1", 0)
        }]
    );
    Ok(())
}
