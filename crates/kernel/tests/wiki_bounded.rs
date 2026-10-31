#![cfg(test)]

use chrono::{DateTime, NaiveDate, Utc};
use kernel::wiki::page::bound::{
    ARCHIVED_TITLE, ARTICLE_BUDGET, HISTORY_LINE, SECTION_BUDGET, SECTIONS_BUDGET, bound_page,
    dropped, fit, history_line, rebase, settle_archive, tail,
};
use kernel::wiki::page::{ArticleSection, WikiHistoryEntry, WikiPage, WikiSlug};
use kernel::wiki::recompile::restore_history;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn at() -> Result<DateTime<Utc>, chrono::ParseError> {
    "2026-10-30T12:00:00Z".parse()
}

fn day(text: &str) -> Result<NaiveDate, chrono::ParseError> {
    text.parse()
}

fn entry(date: &str, title: &str, body: &str) -> Result<WikiHistoryEntry, chrono::ParseError> {
    Ok(WikiHistoryEntry {
        date: day(date)?,
        title: title.to_owned(),
        body: body.to_owned(),
    })
}

fn page() -> Result<WikiPage, Box<dyn std::error::Error>> {
    Ok(WikiPage::empty(
        WikiSlug::try_from("acmekit".to_owned())?,
        "AcmeKit".to_owned(),
    ))
}

fn paragraphs(count: usize, size: usize) -> String {
    (0..count)
        .map(|index| format!("{index:04} {}", "x".repeat(size)))
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[test]
fn fit_keeps_whole_leading_blocks_and_hands_back_the_rest() {
    let text = paragraphs(10, 100);
    let fitted = fit(&text, 330);
    assert_eq!(fitted.kept, paragraphs(3, 100));
    assert!(fitted.overflow.starts_with("0003 "));
    assert_eq!(fit("short", 10).overflow, "");
}

#[test]
fn fit_cuts_a_single_oversized_block_at_a_line() {
    let block = format!("{}\n{}", "a".repeat(60), "b".repeat(60));
    let fitted = fit(&block, 80);
    assert_eq!(fitted.kept, "a".repeat(60));
    assert_eq!(fitted.overflow, "b".repeat(60));
}

#[test]
fn tail_keeps_the_newest_blocks() {
    let text = paragraphs(10, 100);
    let kept = tail(&text, 330);
    assert!(kept.starts_with("0007 "));
    assert!(kept.ends_with(&"x".repeat(100)));
}

#[test]
fn dropped_lists_only_the_blocks_a_rewrite_left_out() {
    let before = "Ships on Fridays.\n\nRuns on the build box.\n\nOld port 8080.";
    let after = "Ships on Fridays.\n\nRuns on the build box.";
    assert_eq!(dropped(before, after), "Old port 8080.");
}

#[test]
fn rebase_takes_the_rewrite_when_the_page_did_not_move() {
    let base = "A.\n\nB.";
    assert_eq!(
        rebase(base, "A, merged.\n\nC.", "A.\n\n  B."),
        "A, merged.\n\nC."
    );
}

#[test]
fn rebase_keeps_a_concurrent_writers_text_and_ours() {
    let base = "Lead.\n\nOld fact.";
    let theirs = "Lead.\n\nOld fact.\n\nTheir new fact.";
    let ours = "Lead.\n\nOur new fact.";
    let merged = rebase(base, ours, theirs);
    assert!(merged.contains("Their new fact."));
    assert!(merged.contains("Our new fact."));
    assert!(!merged.contains("Old fact."));
    assert!(merged.starts_with("Lead."));
}

#[test]
fn history_line_keeps_one_short_line() {
    let long = format!("{}. {}", "First sentence".repeat(10), "y".repeat(400));
    let line = history_line(&long);
    assert!(line.kept.chars().count() <= HISTORY_LINE);
    assert_eq!(line.overflow, long);
    assert_eq!(history_line("Short.").overflow, "");
}

#[test]
fn bound_page_archives_superseded_copies_and_holds_the_budget() -> TestResult {
    let mut previous = page()?;
    previous.history = vec![
        entry("2026-10-20", "Superseded article", "an old copy")?,
        entry("2026-10-19", "Deployed", "Shipped 1.2.")?,
    ];
    let mut next = previous.clone();
    next.article = paragraphs(200, 200);
    next.extra_sections = (0..6)
        .map(|index| ArticleSection {
            heading: format!("Part {index}"),
            body: paragraphs(40, 200),
        })
        .collect();
    let archived = bound_page(Some(&previous), &mut next);
    assert!(next.article.len() <= ARTICLE_BUDGET);
    assert!(
        next.extra_sections
            .iter()
            .all(|section| section.body.len() <= SECTION_BUDGET)
    );
    let total: usize = next.extra_sections.iter().map(|s| s.body.len()).sum();
    assert!(total <= SECTIONS_BUDGET);
    assert!(
        next.history
            .iter()
            .all(|kept| kept.title != "Superseded article")
    );
    assert!(archived.iter().any(|item| item.text == "an old copy"));
    assert!(archived.iter().any(|item| item.label == "Article"));
    Ok(())
}

#[test]
fn settle_archive_adds_one_dated_line_a_day_and_writes_every_block() -> TestResult {
    let mut previous = page()?;
    previous.article = paragraphs(3, 50);
    let mut next = previous.clone();
    next.article = paragraphs(1, 50);
    let mut archived = bound_page(Some(&previous), &mut next);
    let block = settle_archive(&mut next, at()?, Some("0123456789abcdef"), &mut archived)
        .ok_or("an archive block")?;
    assert!(block.contains("<a id=\"a-20261030T120000000Z\"></a>"));
    assert!(block.contains("last inline at 0123456789"));
    let line = next.history.first().ok_or("history line")?;
    assert_eq!(line.title, ARCHIVED_TITLE);
    assert!(
        line.body
            .contains("/wiki/acmekit/archive#a-20261030T120000000Z")
    );
    let again = next.clone();
    next.article = String::new();
    let mut more = bound_page(Some(&again), &mut next);
    assert!(settle_archive(&mut next, at()?, None, &mut more).is_some());
    let lines = next
        .history
        .iter()
        .filter(|kept| kept.title == ARCHIVED_TITLE)
        .count();
    assert_eq!(lines, 1);
    Ok(())
}

#[test]
fn restore_history_puts_real_entries_back_newest_first_within_the_cap() -> TestResult {
    let mut current = page()?;
    current.history = vec![
        entry("2026-10-30", "Superseded article", "copy")?,
        entry("2026-10-28", "Kept", "Still here.")?,
    ];
    let mut recovered = vec![
        entry("2026-10-29", "Pushed out", "Was dropped by a copy.")?,
        entry("2026-10-28", "Kept", "Still  here.")?,
    ];
    recovered.extend(
        (0..60)
            .map(|index| entry("2026-09-01", &format!("Old {index}"), "Older."))
            .collect::<Result<Vec<_>, _>>()?,
    );
    let mut archived = Vec::new();
    let restored = restore_history(&mut current, recovered, &mut archived);
    assert_eq!(current.history.len(), 47);
    let first: Vec<&str> = current
        .history
        .iter()
        .take(2)
        .map(|kept| kept.title.as_str())
        .collect();
    assert_eq!(first, vec!["Pushed out", "Kept"]);
    assert_eq!(restored, 46);
    assert!(archived.iter().any(|item| item.text == "copy"));
    assert_eq!(
        archived
            .iter()
            .filter(|item| item.label.starts_with("History 2026-09-01"))
            .count(),
        15
    );
    Ok(())
}

#[test]
fn the_sections_budget_is_shared_and_uses_what_the_article_leaves() -> TestResult {
    let mut next = page()?;
    next.extra_sections = (0..7)
        .map(|index| ArticleSection {
            heading: format!("Part {index}"),
            body: paragraphs(40, 200),
        })
        .collect();
    next.extra_sections.push(ArticleSection {
        heading: "Small".to_owned(),
        body: "One line.".to_owned(),
    });
    let archived = bound_page(None, &mut next);
    assert!(
        next.extra_sections
            .iter()
            .all(|section| !section.body.is_empty())
    );
    let small = next.extra_sections.last().ok_or("small section")?;
    assert_eq!(small.body, "One line.");
    let total: usize = next.extra_sections.iter().map(|s| s.body.len()).sum();
    assert!(total <= SECTIONS_BUDGET + ARTICLE_BUDGET);
    assert!(total > SECTIONS_BUDGET);
    assert!(archived.iter().any(|item| item.label.starts_with("Part")));
    Ok(())
}

#[test]
fn many_note_sections_drop_the_oldest_whole_instead_of_shredding_all() -> TestResult {
    let mut next = page()?;
    next.extra_sections = (0..200)
        .map(|index| ArticleSection {
            heading: format!("Note {index}"),
            body: paragraphs(3, 200),
        })
        .collect();
    let archived = bound_page(None, &mut next);
    assert!(next.extra_sections.len() < 200);
    assert!(
        next.extra_sections
            .iter()
            .all(|section| section.body.len() >= 400)
    );
    let last = next.extra_sections.last().ok_or("a kept note")?;
    assert_eq!(last.heading, "Note 199");
    assert!(archived.iter().any(|item| item.label == "Note 0"));
    Ok(())
}

#[test]
fn the_entity_list_keeps_its_newest_names() -> TestResult {
    let mut next = page()?;
    next.meta.entities = (0..100).map(|index| format!("Entity {index}")).collect();
    let archived = bound_page(None, &mut next);
    assert_eq!(
        next.meta.entities.len(),
        kernel::wiki::page::bound::ENTITIES
    );
    assert_eq!(
        next.meta.entities.first().map(String::as_str),
        Some("Entity 36")
    );
    let names = archived
        .iter()
        .find(|item| item.label == "Entities")
        .ok_or("archived names")?;
    assert!(names.text.starts_with("Entity 0, Entity 1"));
    Ok(())
}

#[test]
fn restoring_twice_keeps_one_copy_of_a_shortened_entry() -> TestResult {
    let long = format!("Shipped 1.0. {}", "detail ".repeat(80));
    let mut current = page()?;
    current.history = vec![entry(
        "2026-10-28",
        "First release",
        "Shipped 1.0. detail…",
    )?];
    let mut archived = Vec::new();
    let restored = restore_history(
        &mut current,
        vec![entry("2026-10-28", "First release", &long)?],
        &mut archived,
    );
    assert_eq!(restored, 0);
    assert_eq!(current.history.len(), 1);
    Ok(())
}
