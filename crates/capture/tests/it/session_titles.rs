use std::fs;
use std::path::Path;
use std::path::PathBuf;

use capture::listing::StoreRoots;
use capture::listing::StoreSessionLister;
use kernel::transcript::SessionLister;
use kernel::transcript::SessionRenameError;
use serde::Deserialize;
use wire::harness::HarnessId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const U1: &str = "00000000-0000-4000-8000-000000000001";
const PROMPT: &str =
    r#"{"type":"user","message":{"content":"<local-command-caveat>raw</local-command-caveat>"}}"#;

fn write(root: &Path, relative: &str, content: &str) -> Result<PathBuf, std::io::Error> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, content)?;
    Ok(path)
}

fn ai_title(title: &str) -> String {
    format!(r#"{{"type":"ai-title","aiTitle":"{title}","sessionId":"{U1}"}}"#)
}

fn custom_title(title: &str) -> String {
    format!(r#"{{"type":"custom-title","customTitle":"{title}","sessionId":"{U1}"}}"#)
}

async fn claude_title(root: &Path, lines: &[String]) -> Result<String, Box<dyn std::error::Error>> {
    write(
        root,
        &format!("projects/-home-user/{U1}.jsonl"),
        &lines.join("\n"),
    )?;
    let lister = StoreSessionLister::new(StoreRoots {
        claude: Some(root.to_path_buf()),
        ..StoreRoots::default()
    });
    let row = lister
        .describe(HarnessId::ClaudeCode, U1, None)
        .await?
        .ok_or("no row")?;
    Ok(row.title)
}

#[tokio::test]
async fn claude_titles_prefer_manual_then_automatic_then_first_prompt() -> TestResult {
    let dir = tempfile::tempdir()?;
    let prompt = PROMPT.to_owned();
    assert_eq!(
        claude_title(dir.path(), std::slice::from_ref(&prompt)).await?,
        "<local-command-caveat>raw</local-command-caveat>"
    );
    assert_eq!(
        claude_title(
            dir.path(),
            &[prompt.clone(), ai_title("First"), ai_title("Second")]
        )
        .await?,
        "Second"
    );
    assert_eq!(
        claude_title(
            dir.path(),
            &[
                prompt.clone(),
                custom_title("Old name"),
                custom_title("Release notes"),
                ai_title("Later automatic"),
            ]
        )
        .await?,
        "Release notes"
    );
    Ok(())
}

#[tokio::test]
async fn claude_rename_appends_the_custom_title_line() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = write(
        dir.path(),
        &format!("projects/-home-user/{U1}.jsonl"),
        &[PROMPT.to_owned(), ai_title("Automatic")].join("\n"),
    )?;
    let lister = StoreSessionLister::new(StoreRoots {
        claude: Some(dir.path().to_path_buf()),
        ..StoreRoots::default()
    });
    let row = lister
        .rename(HarnessId::ClaudeCode, U1, "Release \"notes\"")
        .await?;
    assert_eq!(row.title, "Release \"notes\"");
    let written = fs::read_to_string(&path)?;
    let expected = format!(
        "{PROMPT}\n{}\n{{\"type\":\"custom-title\",\"customTitle\":\"Release \\\"notes\\\"\",\"sessionId\":\"{U1}\"}}\n",
        ai_title("Automatic")
    );
    assert_eq!(written, expected);
    Ok(())
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct GrokSummary {
    generated_title: String,
    title_is_manual: bool,
    session_summary: String,
    agent_id: String,
}

#[tokio::test]
async fn grok_manual_title_wins_and_rename_marks_it_manual() -> TestResult {
    let dir = tempfile::tempdir()?;
    let summary = write(
        dir.path(),
        "sessions/enc/U1/summary.json",
        r#"{"info":{"id":"U1"},"session_summary":"summary","generated_title":"Generated","updated_at":"2026-10-01T00:00:00Z","agent_id":"keep"}"#,
    )?;
    let lister = StoreSessionLister::new(StoreRoots {
        grok: Some(dir.path().to_path_buf()),
        ..StoreRoots::default()
    });
    let before = lister
        .describe(HarnessId::GrokBuild, "U1", None)
        .await?
        .ok_or("no row")?;
    assert_eq!(before.title, "Generated");
    let renamed = lister.rename(HarnessId::GrokBuild, "U1", "Mine").await?;
    assert_eq!(renamed.title, "Mine");
    let document: GrokSummary = serde_json::from_str(&fs::read_to_string(&summary)?)?;
    assert_eq!(
        document,
        GrokSummary {
            generated_title: "Mine".to_owned(),
            title_is_manual: true,
            session_summary: "summary".to_owned(),
            agent_id: "keep".to_owned(),
        }
    );
    assert!(summary.with_extension("json.lock").is_file());
    Ok(())
}

#[tokio::test]
async fn codex_rename_is_unsupported() -> TestResult {
    let dir = tempfile::tempdir()?;
    let lister = StoreSessionLister::new(StoreRoots {
        codex: Some(dir.path().to_path_buf()),
        ..StoreRoots::default()
    });
    let error = lister.rename(HarnessId::Codex, U1, "name").await;
    assert!(matches!(
        error,
        Err(SessionRenameError::Unsupported(HarnessId::Codex))
    ));
    Ok(())
}
