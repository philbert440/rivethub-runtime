use std::path::Path;

use chrono::{DateTime, Utc};
use kernel::wiki::builder::WikiWriteError;
use kernel::wiki::page::bound::{ARCHIVED_TITLE, ARTICLE_BUDGET};
use kernel::wiki::page::{
    PagePatch, WikiAction, WikiHistoryEntry, WikiPage, WikiSlug, parse_wiki_page,
    serialize_wiki_page,
};
use kernel::wiki::recompile::{RecompileWiki, recompile};
use memory_postgres::PostgresMemory;
use memory_postgres::wiki_writer::{AppliedPage, WikiRepository};
use sqlx::PgPool;
use testkit::wiki_bounded::RecordingMerger;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const SUMMARY_ID: &str = "00000000-0000-4000-8000-000000000001";

fn now() -> Result<DateTime<Utc>, chrono::ParseError> {
    "2026-10-30T12:00:00Z".parse()
}

fn patch(slug: &str, article: &str) -> Result<PagePatch, chrono::ParseError> {
    Ok(PagePatch {
        action: WikiAction::Create,
        slug: slug.to_owned(),
        verified_at: now()?,
        title: Some("AcmeKit".to_owned()),
        add_aliases: Vec::new(),
        add_tags: Vec::new(),
        add_entities: Vec::new(),
        add_related: Vec::new(),
        summary_delta: Some("AcmeKit ships on Fridays.".to_owned()),
        current_state: None,
        article: Some(article.to_owned()),
        article_patches: Vec::new(),
        history_entry: None,
        add_citations: Vec::new(),
        add_sources: Vec::new(),
        allow_shrink: false,
    })
}

async fn apply(root: &Path, patch: PagePatch) -> Result<AppliedPage, WikiWriteError> {
    let (result, released) = {
        let mut repository = WikiRepository::new(root.to_path_buf()).lock().await?;
        let result = repository.apply(patch, SUMMARY_ID, None).await;
        (result, repository.release().await)
    };
    released?;
    result
}

async fn git(root: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = tokio::process::Command::new("git")
        .current_dir(root)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .await?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

fn entry(date: &str, title: &str, body: &str) -> Result<WikiHistoryEntry, chrono::ParseError> {
    Ok(WikiHistoryEntry {
        date: date.parse()?,
        title: title.to_owned(),
        body: body.to_owned(),
    })
}

#[tokio::test]
async fn a_rewrite_archives_what_it_dropped_behind_one_history_line() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("wiki");
    apply(
        &root,
        patch(
            "acmekit",
            "### Releases\n\nShips on Fridays.\n\n### Ports\n\nListens on 8080.",
        )?,
    )
    .await?;
    let inline = git(&root, &["rev-parse", "HEAD"]).await?;
    let applied = apply(
        &root,
        patch("acmekit", "### Releases\n\nShips on Fridays and Tuesdays.")?,
    )
    .await?;
    let page = applied.page;
    assert!(
        page.history
            .iter()
            .all(|kept| kept.title != "Superseded article")
    );
    let line = page.history.first().ok_or("an archive line")?;
    assert_eq!(line.title, ARCHIVED_TITLE);
    assert!(line.body.contains("/wiki/acmekit/archive#a-"));
    assert!(line.body.contains(inline.get(..10).ok_or("sha")?));
    let archive = tokio::fs::read_to_string(root.join("archive/acmekit.md")).await?;
    assert!(archive.contains("Listens on 8080."));
    assert!(archive.starts_with("# Archive: AcmeKit"));
    let files = git(&root, &["show", "--name-only", "--format=", "HEAD"]).await?;
    assert_eq!(files, "archive/acmekit.md\ntopics/acmekit.md\n");
    let stored = tokio::fs::read_to_string(root.join("topics/acmekit.md")).await?;
    assert_eq!(parse_wiki_page(&stored)?, page);
    Ok(())
}

#[tokio::test]
async fn an_oversized_article_is_held_to_its_budget() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("wiki");
    let article = (0..200)
        .map(|index| format!("Fact {index}: {}", "x".repeat(200)))
        .collect::<Vec<_>>()
        .join("\n\n");
    let applied = apply(&root, patch("acmekit", &article)?).await?;
    assert!(applied.page.article.len() <= ARTICLE_BUDGET);
    let archive = tokio::fs::read_to_string(root.join("archive/acmekit.md")).await?;
    assert!(archive.contains("Fact 199:"));
    Ok(())
}

async fn commit_page(root: &Path, page: &WikiPage, message: &str) -> TestResult {
    tokio::fs::create_dir_all(root.join("topics")).await?;
    tokio::fs::write(
        root.join(format!("topics/{}.md", page.meta.slug.as_ref())),
        serialize_wiki_page(page)?,
    )
    .await?;
    git(root, &["add", "--", "topics"]).await?;
    git(root, &["commit", "-q", "-m", message]).await?;
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn recompile_restores_pushed_out_history_and_archives_copies(pool: PgPool) -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("wiki");
    tokio::fs::create_dir_all(&root).await?;
    git(&root, &["init", "-q", "-b", "main"]).await?;
    let slug = WikiSlug::try_from("acmekit".to_owned())?;
    let mut page = WikiPage::empty(slug.clone(), "AcmeKit".to_owned());
    page.current_state = "AcmeKit ships on Fridays.".to_owned();
    page.history = vec![
        entry("2026-10-29", "Moved the build box", "Builds moved.")?,
        entry("2026-10-28", "First release", "Shipped 1.0.")?,
    ];
    commit_page(&root, &page, "real history").await?;
    page.history = (0..4)
        .map(|index| entry("2026-10-30", "Superseded article", &format!("copy {index}")))
        .collect::<Result<Vec<_>, _>>()?;
    page.extra_sections = vec![kernel::wiki::page::ArticleSection {
        heading: "Desktop".to_owned(),
        body: (0..80)
            .map(|index| format!("### Note {index}\n\n{}", "y".repeat(120)))
            .collect::<Vec<_>>()
            .join("\n\n"),
    }];
    commit_page(&root, &page, "copies pushed the history out").await?;
    let memory = PostgresMemory::new(pool.clone()).with_wiki_directory(root.clone());
    let merger = RecordingMerger::default();
    let job = RecompileWiki {
        slug: slug.clone(),
        since: "2026-10-01T00:00:00Z".parse()?,
    };
    let report = recompile(&memory, &merger, &job, now()?)
        .await?
        .ok_or("the page exists")?;
    assert_eq!(report.restored, 2);
    assert_eq!(report.merges, 2);
    let stored =
        parse_wiki_page(&tokio::fs::read_to_string(root.join("topics/acmekit.md")).await?)?;
    let titles: Vec<&str> = stored
        .history
        .iter()
        .map(|kept| kept.title.as_str())
        .collect();
    assert_eq!(
        titles,
        vec![ARCHIVED_TITLE, "Moved the build box", "First release"]
    );
    assert_eq!(stored.current_state, "AcmeKit ships on Fridays. Merged.");
    let archive = tokio::fs::read_to_string(root.join("archive/acmekit.md")).await?;
    assert!(archive.contains("copy 3"));
    assert!(archive.contains("### Note 16"));
    assert!(archive.contains("### Note 79"));
    let sha: String =
        sqlx::query_scalar("SELECT git_sha FROM rh_wiki_topics WHERE slug = 'acmekit'")
            .fetch_one(&pool)
            .await?;
    assert_eq!(sha, git(&root, &["rev-parse", "HEAD"]).await?.trim());
    Ok(())
}
