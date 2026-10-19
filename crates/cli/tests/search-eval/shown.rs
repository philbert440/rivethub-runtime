use std::collections::BTreeMap;

use kernel::memory::SearchOutcome;
use regex::Regex;

const DEGRADED: &str = "⚠ ";
const SUMMARIES: &str = "### Summaries";
const MESSAGES: &str = "### Messages";
const SECTION: &str = "### ";
const HANDLE: &str = "memory_get_full id=";
const ELLIPSIS: char = '…';
const MESSAGE_HEADING: &str =
    r"^- \[[^\]/]+/[^\]]+\](?: \[tool: [^\]]*\])? \(.*?, score: -?\d+\.\d{3}\)";
const LISTED_SUMMARY: &str =
    r"^- \[[^\]/]+/(?P<id>[^\]]+)\] \(.*?, score: -?\d+\.\d{3}\)(?P<rest>.*)$";
const EXPANDED_SUMMARY: &str = r"^\*\*\[[^\]]+\]\*\* \(.*?, score: -?\d+\.\d{3}";

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Shown {
    pub(crate) id: Option<String>,
    pub(crate) text: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Ranking {
    pub(crate) hits: Vec<Shown>,
    pub(crate) degraded: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Reader {
    message: Regex,
    listed: Regex,
    expanded: Regex,
}

enum Section {
    Preamble,
    Summaries,
    Messages,
}

struct Open<'text> {
    id: Option<String>,
    lines: Vec<&'text str>,
}

impl Shown {
    pub(crate) fn is(&self, id: &str, contents: &BTreeMap<String, String>) -> bool {
        self.id.as_deref() == Some(id)
            || contents.get(id).is_some_and(|content| self.shows(content))
    }

    fn shows(&self, content: &str) -> bool {
        if content.is_empty() {
            return false;
        }
        self.text.starts_with(content)
            || self.text.match_indices(ELLIPSIS).any(|(at, _)| {
                self.text
                    .get(..at)
                    .is_some_and(|head| !head.is_empty() && content.starts_with(head))
            })
    }

    fn closed(open: Open<'_>) -> Self {
        let text = open.lines.join("\n").trim_end().to_owned();
        let id = open.id.or_else(|| handle(&text));
        Self { id, text }
    }
}

impl Ranking {
    pub(crate) fn of(outcome: &SearchOutcome) -> Self {
        Self {
            hits: outcome
                .results
                .iter()
                .map(|hit| Shown {
                    id: Some(hit.id.clone()),
                    text: hit.content.clone(),
                })
                .collect(),
            degraded: outcome
                .degraded
                .as_ref()
                .map(|degraded| degraded.reason.clone()),
        }
    }
}

impl Reader {
    pub(crate) fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            message: Regex::new(MESSAGE_HEADING)?,
            listed: Regex::new(LISTED_SUMMARY)?,
            expanded: Regex::new(EXPANDED_SUMMARY)?,
        })
    }

    pub(crate) fn read(&self, text: &str) -> Ranking {
        let mut section = Section::Preamble;
        let mut degraded = None;
        let mut hits = Vec::new();
        let mut open: Option<Open<'_>> = None;
        for line in text.lines() {
            let heading = if line.starts_with(SUMMARIES) {
                section = Section::Summaries;
                None
            } else if line == MESSAGES {
                section = Section::Messages;
                None
            } else {
                match section {
                    Section::Preamble => {
                        if degraded.is_none()
                            && let Some(reason) = line.strip_prefix(DEGRADED)
                        {
                            degraded = Some(reason.to_owned());
                        }
                        None
                    }
                    Section::Summaries => self.summary(line),
                    Section::Messages => self.message.is_match(line).then(|| Open {
                        id: None,
                        lines: Vec::new(),
                    }),
                }
            };
            if let Some(next) = heading {
                hits.extend(open.replace(next).map(Shown::closed));
            } else if let Some(current) = open.as_mut()
                && !line.starts_with(SECTION)
            {
                current.lines.push(line);
            }
        }
        hits.extend(open.map(Shown::closed));
        Ranking { hits, degraded }
    }

    fn summary<'text>(&self, line: &'text str) -> Option<Open<'text>> {
        if let Some(captures) = self.listed.captures(line) {
            let id = captures.name("id").map(|id| id.as_str().to_owned());
            let lines = captures
                .name("rest")
                .map(|rest| rest.as_str().trim_start())
                .into_iter()
                .collect();
            return Some(Open { id, lines });
        }
        self.expanded.is_match(line).then(|| Open {
            id: None,
            lines: Vec::new(),
        })
    }
}

fn handle(text: &str) -> Option<String> {
    let (_, after) = text.split_once(HANDLE)?;
    let id: String = after
        .chars()
        .take_while(|ch| !ch.is_whitespace() && *ch != ']')
        .collect();
    (!id.is_empty()).then_some(id)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::Ranking;
    use super::Reader;
    use super::Shown;

    const RENDERED: &str = "⚠ embedding endpoint unreachable\nkeyword arms only\n\n## Memory Search: \"signing key\"\nFound 3 results (1 summaries, 2 messages)\n\n### Summaries (expanded)\n\n**[leaf]** (2d ago · 2026-10-08 09:00:00 UTC, score: 0.412)\nThe signing key rotates weekly.\n\n  **Source messages (1):**\n  > [user] rotate the signing key\n\n### Messages\n\n- [rivet/user] (2d ago · 2026-10-08 09:01:00 UTC, score: 0.300)\nrotate the signing key\n- [rivet/tool] [tool: shell] (2d ago · 2026-10-08 09:02:00 UTC, score: 0.200)\nkey rotated\n⚠ truncated at capture (full: 9000 chars) → memory_get_full id=00000000-0000-4000-8000-000000000002";

    fn shown(id: Option<&str>, text: &str) -> Shown {
        Shown {
            id: id.map(str::to_owned),
            text: text.to_owned(),
        }
    }

    #[test]
    fn a_rendered_listing_reads_back_in_display_order() -> Result<(), regex::Error> {
        let ranking = Reader::new()?.read(RENDERED);
        assert_eq!(
            ranking,
            Ranking {
                hits: vec![
                    shown(
                        None,
                        "The signing key rotates weekly.\n\n  **Source messages (1):**\n  > [user] rotate the signing key"
                    ),
                    shown(None, "rotate the signing key"),
                    shown(
                        Some("00000000-0000-4000-8000-000000000002"),
                        "key rotated\n⚠ truncated at capture (full: 9000 chars) → memory_get_full id=00000000-0000-4000-8000-000000000002"
                    ),
                ],
                degraded: Some("embedding endpoint unreachable".to_owned()),
            }
        );
        Ok(())
    }

    #[test]
    fn a_listed_summary_carries_its_id_and_inline_content() -> Result<(), regex::Error> {
        let text = "## Memory Search: \"q\"\nFound 1 results (1 summaries, 0 messages)\n\n### Summaries\n\n- [leaf/00000000-0000-4000-8000-400000000001] (1d ago · 2026-10-09 09:00:00 UTC, score: 0.500) tags: project:acme The rollout is blue-green.";
        let ranking = Reader::new()?.read(text);
        assert_eq!(
            ranking.hits,
            [shown(
                Some("00000000-0000-4000-8000-400000000001"),
                "tags: project:acme The rollout is blue-green."
            )]
        );
        Ok(())
    }

    #[test]
    fn an_empty_result_reads_as_no_hits() -> Result<(), regex::Error> {
        let ranking = Reader::new()?.read("No results found for query \"q\".\n\n- [x] retry");
        assert!(ranking.hits.is_empty(), "{ranking:?}");
        assert_eq!(ranking.degraded, None);
        Ok(())
    }

    #[test]
    fn a_hit_without_an_id_is_known_by_the_content_it_shows() {
        let contents = BTreeMap::from([
            ("short".to_owned(), "rotate the signing key".to_owned()),
            ("long".to_owned(), "abcdefghij".to_owned()),
            ("other".to_owned(), "rotate the other key".to_owned()),
        ]);
        let whole = shown(None, "rotate the signing key\n[tool_result]\nok");
        let cut = shown(None, "abcde…");
        assert!(whole.is("short", &contents));
        assert!(cut.is("long", &contents));
        assert!(!whole.is("other", &contents));
        assert!(!whole.is("absent", &contents));
        assert!(!shown(None, "…").is("long", &contents));
        assert!(shown(Some("absent"), "x").is("absent", &contents));
    }
}
