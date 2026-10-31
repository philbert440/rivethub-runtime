use std::collections::BTreeSet;

use chrono::{DateTime, NaiveDate, Utc};

use super::bound::{ARCHIVED_TITLE, Archived, blocks, history_line, spill};
use super::limits;
use super::{WikiHistoryEntry, WikiPage};

pub const ARCHIVE_DIRECTORY: &str = "archive";
const COVERED_PERCENT: usize = 60;
const MONTH: &str = "%Y-%m";

#[must_use]
pub fn archive_month(at: DateTime<Utc>) -> String {
    at.format(MONTH).to_string()
}

#[must_use]
pub fn parse_archive_month(text: &str) -> Option<String> {
    let first = format!("{text}-01");
    let valid = text.len() == 7 && NaiveDate::parse_from_str(&first, "%Y-%m-%d").is_ok();
    valid.then(|| text.to_owned())
}

#[must_use]
pub fn archive_directory(slug: &str) -> String {
    format!("{ARCHIVE_DIRECTORY}/{slug}")
}

#[must_use]
pub fn archive_path(slug: &str, month: &str) -> String {
    format!("{ARCHIVE_DIRECTORY}/{slug}/{month}.md")
}

#[must_use]
pub fn legacy_archive_path(slug: &str) -> String {
    format!("{ARCHIVE_DIRECTORY}/{slug}.md")
}

#[must_use]
pub fn archive_anchor(at: DateTime<Utc>) -> String {
    format!("a-{}", at.format("%Y%m%dT%H%M%S%3fZ"))
}

#[must_use]
pub fn archive_header(title: &str, slug: &str, month: &str) -> String {
    format!(
        "# Archive: {title}, {month}\n\nText moved out of [[{slug}]] in {month} when a rewrite removed it or it went over its size budget, oldest first.\n"
    )
}

#[must_use]
pub fn unarchived(archived: Vec<Archived>, page: &WikiPage, archive: &str) -> Vec<Archived> {
    let mut inline = vec![page.current_state.as_str(), page.article.as_str()];
    inline.extend(
        page.extra_sections
            .iter()
            .map(|section| section.body.as_str()),
    );
    let inline = words(&inline.join("\n\n"));
    let mut stored: Vec<BTreeSet<String>> =
        blocks(archive).iter().map(|block| words(block)).collect();
    let mut kept = Vec::new();
    for item in archived {
        let mut fresh = Vec::new();
        for block in blocks(&item.text) {
            let found = words(&block);
            if covered(&found, &inline) || stored.iter().any(|known| covered(&found, known)) {
                continue;
            }
            stored.push(found);
            fresh.push(block);
        }
        if !fresh.is_empty() {
            kept.push(Archived {
                text: fresh.join("\n\n"),
                ..item
            });
        }
    }
    kept
}

fn words(text: &str) -> BTreeSet<String> {
    text.split(|letter: char| !letter.is_alphanumeric() && !"_.#/-".contains(letter))
        .map(|word| word.trim_matches(|letter: char| !letter.is_alphanumeric()))
        .filter(|word| {
            word.chars().count() >= 4 || word.chars().any(|letter| letter.is_ascii_digit())
        })
        .map(str::to_lowercase)
        .collect()
}

fn covered(found: &BTreeSet<String>, pool: &BTreeSet<String>) -> bool {
    let shared = found.iter().filter(|word| pool.contains(*word)).count();
    shared.saturating_mul(100) >= found.len().saturating_mul(COVERED_PERCENT)
}

#[must_use]
pub fn archive_block(at: DateTime<Utc>, inline: Option<&str>, archived: &[Archived]) -> String {
    let mut block = format!(
        "\n<a id=\"{}\"></a>\n\n## {}",
        archive_anchor(at),
        at.format("%Y-%m-%d %H:%M UTC")
    );
    if let Some(sha) = inline {
        block.push_str(" (last inline at ");
        block.push_str(short(sha));
        block.push(')');
    }
    block.push('\n');
    for item in archived {
        block.push_str("\n### ");
        block.push_str(&item.label);
        block.push_str("\n\n");
        block.push_str(&item.text);
        block.push('\n');
    }
    block
}

#[must_use]
pub fn archive_entry(
    slug: &str,
    at: DateTime<Utc>,
    inline: Option<&str>,
    archived: &[Archived],
) -> WikiHistoryEntry {
    let bytes: usize = archived.iter().map(|item| item.text.len()).sum();
    let mut labels: Vec<&str> = Vec::new();
    for item in archived {
        if !labels.contains(&item.label.as_str()) {
            labels.push(&item.label);
        }
    }
    let named = if labels.len() > 3 {
        format!(
            "{} and {} more",
            labels.get(..3).unwrap_or_default().join(", "),
            labels.len().saturating_sub(3)
        )
    } else {
        labels.join(", ")
    };
    let mut body = format!(
        "Moved {} to [the archive](/wiki/{slug}/archive/{}#{}): {named}.",
        size(bytes),
        archive_month(at),
        archive_anchor(at)
    );
    if let Some(sha) = inline {
        body.push_str(" Last inline at `");
        body.push_str(short(sha));
        body.push_str("`.");
    }
    let line = history_line(&body);
    WikiHistoryEntry {
        date: at.date_naive(),
        title: ARCHIVED_TITLE.to_owned(),
        body: line.kept,
    }
}

pub fn settle_archive(
    page: &mut WikiPage,
    at: DateTime<Utc>,
    inline: Option<&str>,
    archived: &mut Vec<Archived>,
) -> Option<String> {
    if archived.is_empty() {
        return None;
    }
    let today = at.date_naive();
    let noted = page
        .history
        .iter()
        .any(|entry| entry.title == ARCHIVED_TITLE && entry.date == today);
    if archived.iter().any(|item| item.notice) && !noted {
        while page.history.len() >= limits::HISTORY {
            let Some(entry) = page.history.pop() else {
                break;
            };
            spill(archived, &entry);
        }
        let slug = page.meta.slug.as_ref().to_owned();
        page.history
            .insert(0, archive_entry(&slug, at, inline, archived));
    }
    Some(archive_block(at, inline, archived))
}

fn short(sha: &str) -> &str {
    sha.get(..10).unwrap_or(sha)
}

fn size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        let tenths = bytes.saturating_mul(10) / 1024;
        format!("{}.{} KB", tenths / 10, tenths % 10)
    }
}
