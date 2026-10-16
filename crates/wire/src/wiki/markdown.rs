use std::fmt::Write as _;

use strum::IntoEnumIterator as _;

use super::{PageCitation, WikiPage, WikiPageError};
use crate::gateway::wiki::WikiHistoryEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumIter, strum::Display)]
enum Section {
    Summary,
    Article,
    #[strum(serialize = "See also")]
    SeeAlso,
    History,
    Citations,
}

pub fn parse_wiki_page(markdown: &str) -> Result<WikiPage, WikiPageError> {
    let markdown = markdown.replace("\r\n", "\n");
    let body = markdown
        .strip_prefix("---\n")
        .ok_or_else(|| invalid("missing frontmatter"))?;
    let (frontmatter, body) = body
        .split_once("\n---\n")
        .ok_or_else(|| invalid("unterminated frontmatter"))?;
    let mut page = WikiPage {
        frontmatter: serde_yaml_ng::from_str(frontmatter)?,
        preamble: String::new(),
        summary: None,
        article: None,
        see_also: None,
        history: Vec::new(),
        citations: Vec::new(),
    };
    let mut section = None;
    let mut text = String::new();
    let mut seen = Vec::new();
    let mut fence = None;
    for line in body.lines() {
        let heading = if fence.is_none() {
            line.strip_prefix("## ").and_then(section_name)
        } else {
            None
        };
        if let Some(next) = heading {
            if seen.contains(&next) {
                return Err(invalid("duplicate section"));
            }
            set_section(&mut page, section, &text)?;
            text.clear();
            section = Some(next);
            seen.push(next);
        } else {
            text.push_str(line);
            text.push('\n');
        }
        update_fence(&mut fence, line);
    }
    set_section(&mut page, section, &text)?;
    Ok(page)
}

fn section_name(text: &str) -> Option<Section> {
    Section::iter().find(|section| section.to_string() == text)
}

fn update_fence(fence: &mut Option<(char, usize)>, line: &str) {
    let line = line.trim_start();
    if let Some((marker, length)) = *fence {
        let count = line.chars().take_while(|ch| *ch == marker).count();
        if count >= length && line.trim_matches(marker).trim().is_empty() {
            *fence = None;
        }
    } else if let Some(marker @ ('`' | '~')) = line.chars().next() {
        let count = line.chars().take_while(|ch| *ch == marker).count();
        if count >= 3 {
            *fence = Some((marker, count));
        }
    }
}

fn set_section(
    page: &mut WikiPage,
    section: Option<Section>,
    text: &str,
) -> Result<(), WikiPageError> {
    let text = text.trim().to_owned();
    match section {
        None => page.preamble = text,
        Some(Section::Summary) => page.summary = Some(text),
        Some(Section::Article) => page.article = Some(text),
        Some(Section::SeeAlso) => page.see_also = Some(text),
        Some(Section::History) => page.history = parse_history(&text)?,
        Some(Section::Citations) => page.citations = parse_citations(&text)?,
    }
    Ok(())
}

fn parse_history(text: &str) -> Result<Vec<WikiHistoryEntry>, WikiPageError> {
    let mut entries: Vec<WikiHistoryEntry> = Vec::new();
    let mut fence = None;
    for line in text.lines() {
        if let Some(heading) = line.strip_prefix("### ").filter(|_| fence.is_none()) {
            let (date, title) = heading
                .split_once(" — ")
                .ok_or_else(|| invalid("history heading"))?;
            entries.push(WikiHistoryEntry {
                date: date.parse()?,
                title: title.to_owned(),
                body: String::new(),
            });
        } else if let Some(entry) = entries.last_mut() {
            entry.body.push_str(line);
            entry.body.push('\n');
        } else if !line.trim().is_empty() {
            return Err(invalid("history without dated entry"));
        }
        update_fence(&mut fence, line);
    }
    for entry in &mut entries {
        entry.body = entry.body.trim().to_owned();
    }
    Ok(entries)
}

fn parse_citations(text: &str) -> Result<Vec<PageCitation>, WikiPageError> {
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut lines = text.lines();
    let header = lines.next().ok_or_else(|| invalid("citation header"))?;
    if cells(header)? != ["Date", "Kind", "Summary", "Note"] {
        return Err(invalid("citation columns"));
    }
    let separator = lines.next().ok_or_else(|| invalid("citation separator"))?;
    if !cells(separator)?
        .iter()
        .all(|cell| !cell.is_empty() && cell.chars().all(|ch| ch == '-' || ch == ':'))
    {
        return Err(invalid("citation separator"));
    }
    lines
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields = cells(line)?;
            let [date, kind, summary_id, note] = fields.as_slice() else {
                return Err(invalid("citation row"));
            };
            Ok(PageCitation {
                date: date.parse()?,
                kind: kind.parse()?,
                summary_id: summary_id.trim_matches('`').to_owned(),
                note: note.clone(),
            })
        })
        .collect()
}

fn cells(line: &str) -> Result<Vec<String>, WikiPageError> {
    let content = line
        .trim()
        .strip_prefix('|')
        .and_then(|line| line.strip_suffix('|'))
        .ok_or_else(|| invalid("citation table"))?;
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut escaped = false;
    for ch in content.chars() {
        if escaped {
            cell.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '|' {
            cells.push(cell.trim().replace("<br>", "\n"));
            cell.clear();
        } else {
            cell.push(ch);
        }
    }
    if escaped {
        return Err(invalid("citation escape"));
    }
    cells.push(cell.trim().replace("<br>", "\n"));
    Ok(cells)
}

pub fn serialize_wiki_page(page: &WikiPage) -> Result<String, WikiPageError> {
    let frontmatter = serde_yaml_ng::to_string(&page.frontmatter)?;
    let mut output = format!("---\n{frontmatter}---\n");
    if !page.preamble.is_empty() {
        write!(output, "\n{}\n", page.preamble).map_err(format_error)?;
    }
    for (section, text) in [
        (Section::Summary, &page.summary),
        (Section::Article, &page.article),
        (Section::SeeAlso, &page.see_also),
    ] {
        if let Some(text) = text {
            write!(output, "\n## {section}\n\n{text}\n").map_err(format_error)?;
        }
    }
    if !page.history.is_empty() {
        output.push_str("\n## History\n");
        for entry in &page.history {
            write!(
                output,
                "\n### {} — {}\n\n{}\n",
                entry.date, entry.title, entry.body
            )
            .map_err(format_error)?;
        }
    }
    if !page.citations.is_empty() {
        output.push_str("\n## Citations\n\n| Date | Kind | Summary | Note |\n|------|------|---------|------|\n");
        for citation in &page.citations {
            writeln!(
                output,
                "| {} | {} | `{}` | {} |",
                citation.date,
                citation.kind,
                escape_cell(&citation.summary_id),
                escape_cell(&citation.note)
            )
            .map_err(format_error)?;
        }
    }
    Ok(output)
}

fn escape_cell(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "<br>")
}

fn invalid(message: &str) -> WikiPageError {
    WikiPageError::Format(message.to_owned())
}

fn format_error(error: std::fmt::Error) -> WikiPageError {
    WikiPageError::Format(error.to_string())
}
