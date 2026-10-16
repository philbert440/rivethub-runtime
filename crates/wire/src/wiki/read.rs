use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::limits;
use super::serialize::{CITATION_COLUMNS, citation_row, history_heading};
use super::summary::cap_summary;
use super::{PageCitation, WikiPage, parse_wiki_page};
use crate::gateway::wiki::WikiHistoryEntry;

pub const READ_LIMIT: usize = 24_000;
const RAW_PREVIEW: usize = 8_000;
const ARTICLE_PREVIEW: usize = 8_000;
const HEADER_ALIASES: usize = 8;
const HEADER_TAGS: usize = 16;
const ALIAS_PAGE: usize = 200;
const SEE_ALSO_PREVIEW: usize = 24;
const RECENT_HISTORY: usize = 10;
const SLICE_HINT: &str = "Use section=summary|article|history|aliases|citations for a slice. section=full is refused while the page exceeds 24,000 characters.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, strum::Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ReadSection {
    Summary,
    Article,
    History,
    Aliases,
    Citations,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("section=full is refused while the page exceeds 24,000 characters")]
pub struct FullPageRefused;

pub fn format_wiki_read(
    markdown: &str,
    slug: &str,
    section: Option<ReadSection>,
) -> Result<String, FullPageRefused> {
    let size = markdown.chars().count();
    let oversized = size > READ_LIMIT;
    let page = match parse_wiki_page(markdown) {
        Ok(page) => page,
        Err(error) => return Ok(malformed(markdown, slug, &error.to_string(), size)),
    };
    let (section, body) = match section {
        None | Some(ReadSection::Full) if !oversized => return Ok(markdown.to_owned()),
        None => return Ok(encyclopedia(&page, slug, size)),
        Some(ReadSection::Full) => return Err(FullPageRefused),
        Some(section @ ReadSection::Summary) => (section, summary(&page)),
        Some(section @ ReadSection::Article) => (
            section,
            format!("## Article\n\n{}", or_empty(&page.article)),
        ),
        Some(section @ ReadSection::History) => (section, history(&page.history)),
        Some(section @ ReadSection::Aliases) => (section, aliases(&page.meta.aliases, slug)),
        Some(section @ ReadSection::Citations) => (section, citations(&page.citations)),
    };
    let lead = if oversized {
        format!("{} section={section}", oversized_banner(&page, slug, size))
    } else {
        format!("section={section}")
    };
    Ok(format!(
        "{lead}\n\n{}\n\n{body}",
        header(&page, section == ReadSection::Aliases)
    ))
}

fn malformed(markdown: &str, slug: &str, error: &str, size: usize) -> String {
    let banner = format!(
        "⚠ Page \"{slug}\" is malformed ({error}) — raw content follows; the next extractor pass will re-structure it."
    );
    if size <= READ_LIMIT {
        return format!("{banner}\n\n{markdown}");
    }
    let preview: String = markdown.chars().take(RAW_PREVIEW).collect();
    format!(
        "{banner}\nPage is also oversized ({} chars); showing the first {}.\n\n{preview}\n\n…[{} more chars — wiki_read slug={slug} after the next extractor pass]",
        grouped(size),
        grouped(RAW_PREVIEW),
        grouped(size - RAW_PREVIEW)
    )
}

fn oversized_banner(page: &WikiPage, slug: &str, size: usize) -> String {
    format!(
        "⚠ Oversized wiki page \"{slug}\" ({} KB · {} aliases · {} history · {} citations · {} see-also).",
        kilobytes(size),
        page.meta.aliases.len(),
        page.history.len(),
        page.citations.len(),
        page.see_also.len()
    )
}

fn encyclopedia(page: &WikiPage, slug: &str, size: usize) -> String {
    let mut parts = vec![
        format!(
            "{}\nThe raw file leads with YAML aliases, so MCP/capture truncation hid Summary and Article.\nShowing the encyclopedia view (lead + article + recent history), not the raw file.",
            oversized_banner(page, slug, size)
        ),
        header(page, false),
        summary(page),
    ];
    if !page.article.trim().is_empty() {
        parts.push(format!(
            "## Article\n\n{}",
            article_preview(&page.article, slug)
        ));
    }
    if !page.see_also.is_empty() {
        parts.push(see_also(&page.see_also, slug));
    }
    let recent = limits::first(&page.history, RECENT_HISTORY);
    parts.push(format!(
        "## Recent history ({} of {})\n\n{}",
        recent.len(),
        page.history.len(),
        history_entries(recent)
    ));
    if !page.citations.is_empty() {
        parts.push(citations(&page.citations));
    }
    parts.push(SLICE_HINT.to_owned());
    parts.join("\n\n")
}

fn header(page: &WikiPage, alias_slice: bool) -> String {
    let mut lines = vec![
        "---".to_owned(),
        format!("title: {}", page.meta.title),
        format!("slug: {}", page.meta.slug.as_ref()),
    ];
    if let Some(verified) = &page.meta.last_verified {
        lines.push(format!("last_verified: {}", verified.as_ref()));
    }
    let tags = &page.meta.tags;
    if !tags.is_empty() {
        let hidden =
            (tags.len() > HEADER_TAGS).then(|| format!(" (+{})", tags.len() - HEADER_TAGS));
        lines.push(format!(
            "tags: {}{}",
            limits::first(tags, HEADER_TAGS).join(", "),
            hidden.unwrap_or_default()
        ));
    }
    let aliases = &page.meta.aliases;
    lines.push(if alias_slice {
        format!("aliases_count: {}", aliases.len())
    } else if aliases.is_empty() {
        "aliases: 0".to_owned()
    } else if aliases.len() > HEADER_ALIASES {
        format!(
            "aliases: {} (showing {HEADER_ALIASES}: {}) — section=aliases",
            aliases.len(),
            limits::first(aliases, HEADER_ALIASES).join(", ")
        )
    } else {
        format!("aliases: {} ({})", aliases.len(), aliases.join(", "))
    });
    lines.push("---".to_owned());
    lines.join("\n")
}

fn summary(page: &WikiPage) -> String {
    format!("## Summary\n\n{}", or_empty(&page.current_state))
}

fn history(entries: &[WikiHistoryEntry]) -> String {
    format!(
        "## History ({} of {}, newest first)\n\n{}",
        entries.len(),
        entries.len(),
        history_entries(entries)
    )
}

fn history_entries(entries: &[WikiHistoryEntry]) -> String {
    if entries.is_empty() {
        return "(none yet)".to_owned();
    }
    entries
        .iter()
        .map(|entry| format!("{}\n\n{}", history_heading(entry), or_empty(&entry.body)))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn aliases(aliases: &[String], slug: &str) -> String {
    if aliases.is_empty() {
        return "## Aliases (0 of 0)\n\n(none)".to_owned();
    }
    let shown = limits::first(aliases, ALIAS_PAGE);
    let mut lines: Vec<String> = shown.iter().map(|alias| format!("- {alias}")).collect();
    if aliases.len() > ALIAS_PAGE {
        lines.push(format!(
            "…[{} more — wiki_read slug={slug} section=aliases (first {ALIAS_PAGE} only)]",
            aliases.len() - ALIAS_PAGE
        ));
    }
    format!(
        "## Aliases ({} of {})\n\n{}",
        shown.len(),
        aliases.len(),
        lines.join("\n")
    )
}

fn citations(citations: &[PageCitation]) -> String {
    if citations.is_empty() {
        return "## Citations (0 of 0)\n\n(none)".to_owned();
    }
    let rows: Vec<String> = citations.iter().map(citation_row).collect();
    format!(
        "## Citations ({} of {})\n\n{CITATION_COLUMNS}\n{}",
        citations.len(),
        citations.len(),
        rows.join("\n")
    )
}

fn see_also(links: &[String], slug: &str) -> String {
    let mut items: Vec<String> = limits::first(links, SEE_ALSO_PREVIEW)
        .iter()
        .map(|link| format!("- [[{link}]]"))
        .collect();
    if links.len() > SEE_ALSO_PREVIEW {
        items.push(format!(
            "…[{} more — wiki_read slug={slug} (see-also is not a slice; pick a linked slug)]",
            links.len() - SEE_ALSO_PREVIEW
        ));
    }
    format!("## See also\n\n{}", items.join("\n"))
}

fn article_preview(article: &str, slug: &str) -> String {
    let capped = cap_summary(article, ARTICLE_PREVIEW);
    if capped.overflow.is_empty() {
        return capped.kept;
    }
    format!(
        "{}\n\n…[{} more chars — wiki_read slug={slug} section=article]",
        capped.kept,
        grouped(article.chars().count() - capped.kept.chars().count())
    )
}

fn or_empty(text: &str) -> &str {
    if text.trim().is_empty() {
        "(empty)"
    } else {
        text
    }
}

fn kilobytes(size: usize) -> String {
    let tenths = (size * 10 + 512) / 1024;
    format!("{}.{}", tenths / 10, tenths % 10)
}

fn grouped(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}
