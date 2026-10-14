use crate::{Error, json, text};
use kernel::memory::MemoryStore;
use kernel::wiki::{WikiFilter, WikiLookup};
use rmcp::model::{CallToolResult, ToolAnnotations};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashSet;
use wire::gateway::wiki::{WikiPageResponse, WikiSlug};

const PAGE_LIMIT: usize = 24_000;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(!deny_unknown_fields)]
pub struct Search {
    #[schemars(description = "Topic to look for (name, alias, or content terms)")]
    query: String,
    #[schemars(description = "Max results (default 5)", range(min = 1, max = 20), transform = crate::catalog::optional_non_null)]
    limit: Option<u32>,
}

impl crate::catalog::ToolInput for Search {
    const NAME: &'static str = "wiki_search";
    const DESCRIPTION: &'static str = "Search the RivetHub memory wiki — curated topic pages distilled from conversation memory (\"what is currently true about X\"). Higher signal than memory_search for standing facts about projects, hosts, and services; use memory_search when you need what was actually said. Returns slugs — read the page (dated history + provenance) with wiki_read.";

    fn annotations() -> ToolAnnotations {
        ToolAnnotations::from_raw(None, Some(true), None, Some(true), None)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(!deny_unknown_fields)]
pub struct Read {
    #[schemars(description = "Topic slug, e.g. rh-task-engine", with = "String")]
    slug: WikiSlug,
    #[schemars(description = "Slice of an oversized page. Default: verbatim when small, encyclopedia view (Summary + Article + recent history) when large. full is refused while the page exceeds 24,000 characters.", transform = crate::catalog::optional_non_null)]
    section: Option<Section>,
}

impl crate::catalog::ToolInput for Read {
    const NAME: &'static str = "wiki_read";
    const DESCRIPTION: &'static str = "Read one RivetHub wiki topic page: Wikipedia-style Summary (lead), Article body, See also crosslinks, dated History, and Citations (summary UUIDs usable with memory tools for drill-down). Use the slug from wiki_search. Small pages are returned verbatim. Pages larger than 24,000 characters (hub topics with thousands of YAML aliases) return a bounded encyclopedia view so MCP/capture truncation cannot hide Summary behind the alias dump. Pass section=summary|article|history|aliases|citations for a slice; section=full is refused on oversized pages.";

    fn annotations() -> ToolAnnotations {
        ToolAnnotations::from_raw(None, Some(true), None, Some(true), None)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
enum Section {
    Summary,
    Article,
    History,
    Aliases,
    Citations,
    Full,
}

pub async fn search(store: &dyn MemoryStore, args: Search) -> Result<CallToolResult, Error> {
    json(
        &store
            .wiki_topics(WikiFilter {
                query: Some(args.query),
                tag: None,
                limit: crate::memory::limit(args.limit, 5, 20)?.get(),
                offset: 0,
            })
            .await?,
    )
}

pub async fn read(store: &dyn MemoryStore, args: Read) -> Result<CallToolResult, Error> {
    let page = lookup(store, args.slug).await?;
    let oversized = page.markdown.chars().count() > PAGE_LIMIT;
    let content = match args.section {
        Some(Section::Full) if oversized => {
            return Err(Error::Refused(
                "wiki page exceeds 24,000 characters; choose a section",
            ));
        }
        None if oversized => encyclopedia(&page),
        None | Some(Section::Full) => page.markdown,
        Some(Section::Summary) => bounded(&page.current_state, PAGE_LIMIT),
        Some(Section::Article) => match page.article {
            Some(article) => bounded(&article, PAGE_LIMIT),
            None => return Err(Error::Refused("wiki page has no article section")),
        },
        Some(Section::History) => bounded(&history(&page), PAGE_LIMIT),
        Some(Section::Aliases) => bounded(&page.aliases.join("\n"), PAGE_LIMIT),
        Some(Section::Citations) => bounded(
            &page
                .citations
                .iter()
                .map(|citation| {
                    let mut line = citation.summary_id.clone();
                    for detail in [&citation.date, &citation.kind, &citation.note]
                        .into_iter()
                        .flatten()
                    {
                        line.push_str(" — ");
                        line.push_str(detail);
                    }
                    line
                })
                .collect::<Vec<_>>()
                .join("\n"),
            PAGE_LIMIT,
        ),
    };
    Ok(text(content))
}

async fn lookup(store: &dyn MemoryStore, mut slug: WikiSlug) -> Result<WikiPageResponse, Error> {
    let mut visited = HashSet::new();
    for _ in 0..16 {
        if !visited.insert(slug.as_ref().to_owned()) {
            return Err(Error::WikiRedirect);
        }
        match store.wiki_page(&slug).await? {
            WikiLookup::Page(page) => return Ok(*page),
            WikiLookup::Redirect(target) => slug = WikiSlug::try_from(target)?,
            WikiLookup::Missing => return Err(Error::WikiMissing(slug.into())),
        }
    }
    Err(Error::WikiRedirect)
}

fn bounded(value: &str, limit: usize) -> String {
    const NOTICE: &str = "\n…[truncated; select a wiki_read section]";
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let mut result: String = value.chars().take(limit - NOTICE.chars().count()).collect();
    result.push_str(NOTICE);
    result
}

fn history(page: &WikiPageResponse) -> String {
    let mut entries: Vec<_> = page.history.iter().collect();
    entries.sort_by(|a, b| b.date.cmp(&a.date));
    entries
        .into_iter()
        .map(|entry| format!("### {} — {}\n{}", entry.date, entry.title, entry.body))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn encyclopedia(page: &WikiPageResponse) -> String {
    let mut result = format!(
        "# {}\n\n## Summary\n{}",
        bounded(&page.title, 256),
        bounded(&page.current_state, 6_000),
    );
    if let Some(article) = &page.article {
        result.push_str("\n\n## Article\n");
        result.push_str(&bounded(article, 12_000));
    }
    if let Some(links) = &page.see_also {
        result.push_str("\n\n## See also\n");
        result.push_str(&bounded(&links.join("\n"), 1_000));
    }
    result.push_str("\n\n## History\n");
    result.push_str(&bounded(&history(page), 4_000));
    result.push_str(
        "\n\nBounded wiki view; use section=summary|article|history|aliases|citations for a slice.",
    );
    result
}
