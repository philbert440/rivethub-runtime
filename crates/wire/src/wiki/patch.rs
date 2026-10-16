use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{PageCitation, WikiFrontmatter, WikiPage};
use crate::Extra;
use crate::gateway::wiki::{WikiHistoryEntry, WikiSlug, WikiSource};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, strum::Display)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum WikiAction {
    Create,
    Update,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WikiPatch {
    pub action: WikiAction,
    pub slug: WikiSlug,
    pub title: String,
    pub aliases: Vec<WikiSlug>,
    pub tags: Vec<String>,
    pub entities: Vec<String>,
    pub summary: String,
    pub article: Option<String>,
    pub related: Option<Vec<WikiSlug>>,
    pub history_entry: Option<WikiHistoryEntry>,
}

pub fn apply_patch(
    page: Option<WikiPage>,
    patch: WikiPatch,
    citation: PageCitation,
    source: WikiSource,
    verified: DateTime<Utc>,
) -> WikiPage {
    let mut page = match page {
        Some(page) => page,
        None => WikiPage {
            frontmatter: WikiFrontmatter {
                title: patch.title.clone(),
                slug: patch.slug.clone(),
                aliases: Vec::new(),
                tags: Vec::new(),
                entities: Vec::new(),
                last_verified: None,
                sources: Vec::new(),
                extra: Extra::default(),
            },
            preamble: String::new(),
            summary: None,
            article: None,
            see_also: None,
            history: Vec::new(),
            citations: Vec::new(),
        },
    };
    page.frontmatter.title = patch.title;
    for alias in patch.aliases {
        let alias = String::from(alias);
        if alias != page.frontmatter.slug.as_ref() && !page.frontmatter.aliases.contains(&alias) {
            page.frontmatter.aliases.push(alias);
        }
    }
    merge(&mut page.frontmatter.tags, patch.tags);
    merge(&mut page.frontmatter.entities, patch.entities);
    page.summary = Some(patch.summary);
    if let Some(article) = patch.article {
        page.article = Some(article);
    }
    if let Some(related) = patch.related {
        page.see_also = Some(
            related
                .into_iter()
                .map(|slug| format!("- [[{}]]", slug.as_ref()))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    if let Some(entry) = patch.history_entry
        && !page.history.contains(&entry)
    {
        page.history.push(entry);
    }
    if !page
        .citations
        .iter()
        .any(|entry| entry.summary_id == citation.summary_id)
    {
        page.citations.push(citation);
        page.frontmatter.last_verified = Some(verified);
    }
    if !page.frontmatter.sources.contains(&source) {
        page.frontmatter.sources.push(source);
    }
    page
}

fn merge(target: &mut Vec<String>, incoming: Vec<String>) {
    for item in incoming {
        if !target.contains(&item) {
            target.push(item);
        }
    }
}

pub fn extract_wiki_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = text;
    while let Some((_, tail)) = rest.split_once("[[") {
        let Some((link, tail)) = tail.split_once("]]") else {
            break;
        };
        let target = link
            .split(['|', '#'])
            .next()
            .filter(|target| !target.trim().is_empty());
        if let Some(target) = target {
            let target = target.trim().to_owned();
            if !links.contains(&target) {
                links.push(target);
            }
        }
        rest = tail;
    }
    links
}

pub fn build_wiki_search_text(page: &WikiPage) -> String {
    let mut parts = vec![page.frontmatter.title.clone(), page.preamble.clone()];
    parts.extend(page.frontmatter.aliases.iter().cloned());
    parts.extend(page.frontmatter.tags.iter().cloned());
    parts.extend(page.frontmatter.entities.iter().cloned());
    parts.extend(page.summary.iter().cloned());
    parts.extend(page.article.iter().cloned());
    parts.extend(page.see_also.iter().cloned());
    for entry in &page.history {
        parts.push(entry.title.clone());
        parts.push(entry.body.clone());
    }
    parts.retain(|part| !part.is_empty());
    parts.join("\n")
}
