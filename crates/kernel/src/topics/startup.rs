use std::collections::HashSet;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use wire::gateway::memory::MemoryHitSource;
use wire::gateway::memory::MemoryScope;
use wire::gateway::memory::SummaryKind;
use wire::gateway::topics as dto;
pub use wire::gateway::topics::StartupKind;
use wire::gateway::wiki::WikiSlug;
use wire::memory::MemorySearchResult;
use wire::session_id::SessionId;

use super::PlacedSession;
use super::Topic;
use super::TopicError;
use super::TopicId;
use super::TopicPageError;
use super::TopicPageStore;
use super::TopicStore;
use crate::memory::Limit;
use crate::memory::MemoryStore;
use crate::memory::SearchMode;
use crate::memory::SearchQuery;
use crate::memory::budget::char_prefix;
use crate::memory::budget::estimate_tokens;
use crate::memory::budget::relative_when;
use crate::memory::summary_title;

pub const STARTUP_BUDGET: usize = 4_000;
const TOPIC_SHARE: usize = 1_600;
const RECENT_SHARE: usize = 1_000;
const RELEVANT_SHARE: usize = 1_400;
const RECENT_SESSIONS: usize = 4;
const RELEVANT_HITS: u32 = 10;
const ARTICLE_START: usize = 2_400;
const RECENT_CAP: usize = 900;
const RELEVANT_CAP: usize = 500;
const SHORTEST_CLIP: usize = 80;
const SEPARATOR: &str = "\n\n";

const fn share(kind: StartupKind) -> usize {
    match kind {
        StartupKind::Topic => TOPIC_SHARE,
        StartupKind::Recent => RECENT_SHARE,
        StartupKind::Relevant => RELEVANT_SHARE,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupSource {
    Session(SessionId),
    Wiki(WikiSlug),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupSection {
    pub kind: StartupKind,
    pub title: String,
    pub text: String,
    pub sources: Vec<StartupSource>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupContext {
    pub topic: TopicId,
    pub sections: Vec<StartupSection>,
    pub tokens: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupEntry {
    pub text: String,
    pub source: Option<StartupSource>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupPart {
    pub title: String,
    pub entries: Vec<StartupEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupParts {
    pub topic: TopicId,
    pub page: StartupPart,
    pub recent: StartupPart,
    pub relevant: StartupPart,
}

#[async_trait]
pub trait StartupStore: Send + Sync {
    async fn keep_startup(
        &self,
        session: &SessionId,
        context: &StartupContext,
        now: DateTime<Utc>,
    ) -> Result<(), TopicError>;

    async fn startup(&self, session: &SessionId) -> Result<Option<StartupContext>, TopicError>;
}

#[must_use]
pub fn assemble(parts: StartupParts, budget: usize) -> StartupContext {
    let ordered = [
        (StartupKind::Topic, parts.page),
        (StartupKind::Recent, parts.recent),
        (StartupKind::Relevant, parts.relevant),
    ];
    let mut reach = 0_usize;
    let mut used = 0_usize;
    let mut sections = Vec::new();
    for (kind, part) in ordered {
        reach += share(kind);
        let cap =
            budget / STARTUP_BUDGET * reach + budget % STARTUP_BUDGET * reach / STARTUP_BUDGET;
        let allowance = cap.saturating_sub(used);
        if let Some((section, spent)) = fill(kind, part, allowance) {
            used += spent;
            sections.push(section);
        }
    }
    StartupContext {
        topic: parts.topic,
        sections,
        tokens: used,
    }
}

fn fill(kind: StartupKind, part: StartupPart, allowance: usize) -> Option<(StartupSection, usize)> {
    let heading = estimate_tokens(&part.title);
    let limit = allowance.checked_sub(heading)?.saturating_mul(4);
    let mut text = String::new();
    let mut length = 0_usize;
    let mut sources = Vec::new();
    for entry in part.entries {
        let body = entry.text.trim();
        if body.is_empty() {
            continue;
        }
        let gap = if text.is_empty() { 0 } else { SEPARATOR.len() };
        let free = limit.saturating_sub(length + gap);
        let whole = body.chars().count();
        let kept = if whole <= free {
            body.to_owned()
        } else if free >= SHORTEST_CLIP {
            format!("{}\u{2026}", char_prefix(body, free - 1).trim_end())
        } else {
            break;
        };
        if gap > 0 {
            text.push_str(SEPARATOR);
        }
        length += gap + kept.chars().count();
        text.push_str(&kept);
        if let Some(source) = entry.source
            && !sources.contains(&source)
        {
            sources.push(source);
        }
        if whole > free {
            break;
        }
    }
    if text.is_empty() {
        return None;
    }
    let spent = heading + estimate_tokens(&text);
    Some((
        StartupSection {
            kind,
            title: part.title,
            text,
            sources,
        },
        spent,
    ))
}

pub async fn startup_context(
    topics: &dyn TopicStore,
    pages: &dyn TopicPageStore,
    memory: &dyn MemoryStore,
    topic: &Topic,
    prompt: Option<&str>,
    now: DateTime<Utc>,
) -> Result<StartupContext, TopicPageError> {
    let placed = topics.sessions(topic.id).await?;
    let page = page_part(topics, pages, topic).await?;
    let recent = recent_part(memory, topic, &placed, now).await?;
    let relevant = match prompt.map(str::trim).filter(|prompt| !prompt.is_empty()) {
        Some(prompt) => relevant_part(memory, prompt, &placed, &recent, now).await?,
        None => StartupPart::default(),
    };
    Ok(assemble(
        StartupParts {
            topic: topic.id,
            page,
            recent,
            relevant,
        },
        STARTUP_BUDGET,
    ))
}

async fn page_part(
    topics: &dyn TopicStore,
    pages: &dyn TopicPageStore,
    topic: &Topic,
) -> Result<StartupPart, TopicPageError> {
    let mut part = StartupPart {
        title: topic.name.as_str().to_owned(),
        entries: Vec::new(),
    };
    let Some(slug) = topics.page_slug(topic.root()).await? else {
        return Ok(part);
    };
    let Some(page) = pages.topic_page(&slug).await? else {
        return Ok(part);
    };
    let source = Some(StartupSource::Wiki(slug));
    let section = page
        .sections
        .iter()
        .filter(|section| topic.parent.is_some() && section.topic == Some(topic.id))
        .map(|section| section.body.clone());
    let lead = [
        page.summary.clone(),
        char_prefix(&page.article, ARTICLE_START).to_owned(),
    ];
    part.entries = section
        .chain(lead)
        .map(|text| StartupEntry {
            text,
            source: source.clone(),
        })
        .collect();
    Ok(part)
}

async fn recent_part(
    memory: &dyn MemoryStore,
    topic: &Topic,
    placed: &[PlacedSession],
    now: DateTime<Utc>,
) -> Result<StartupPart, TopicPageError> {
    let mut entries = Vec::new();
    for session in placed {
        if entries.len() == RECENT_SESSIONS {
            break;
        }
        let id = &session.placement.session;
        let latest = memory
            .summary_dag(id.as_str())
            .await?
            .into_iter()
            .filter(|node| node.kind == SummaryKind::Leaf && !node.content.trim().is_empty())
            .max_by_key(|node| node.latest_at.unwrap_or(node.created_at));
        let Some(leaf) = latest else {
            continue;
        };
        let content = leaf.content.trim();
        let title = summary_title(content).unwrap_or_else(|| id.to_string());
        let heading = match session.last_active {
            Some(at) => format!("{title} ({})", relative_when(now - at)),
            None => title,
        };
        entries.push(StartupEntry {
            text: format!("{heading}\n{}", char_prefix(content, RECENT_CAP)),
            source: Some(StartupSource::Session(id.clone())),
        });
    }
    Ok(StartupPart {
        title: format!("Recent in {}", topic.name.as_str()),
        entries,
    })
}

async fn relevant_part(
    memory: &dyn MemoryStore,
    prompt: &str,
    placed: &[PlacedSession],
    recent: &StartupPart,
    now: DateTime<Utc>,
) -> Result<StartupPart, TopicPageError> {
    let outcome = memory
        .search(SearchQuery {
            text: prompt.to_owned(),
            mode: SearchMode::Hybrid,
            scope: MemoryScope::Both,
            agent: None,
            role: None,
            tool_name: None,
            tag: None,
            limit: Limit::clamped(Some(RELEVANT_HITS), RELEVANT_HITS, 1..=RELEVANT_HITS),
            since: None,
            before: None,
            window: None,
            expand: None,
            synthesize: None,
        })
        .await?;
    let in_topic: HashSet<&str> = placed
        .iter()
        .map(|session| session.placement.session.as_str())
        .collect();
    let mut hits = outcome.results;
    hits.sort_by_key(|hit| {
        !hit.session_id
            .as_deref()
            .is_some_and(|session| in_topic.contains(session))
    });
    let entries = hits
        .into_iter()
        .filter(|hit| {
            let content = hit.content.trim();
            !content.is_empty()
                && !recent
                    .entries
                    .iter()
                    .any(|entry| entry.text.contains(char_prefix(content, RECENT_CAP)))
        })
        .map(|hit| relevant_entry(&hit, now))
        .collect();
    Ok(StartupPart {
        title: "Relevant memory".to_owned(),
        entries,
    })
}

fn relevant_entry(hit: &MemorySearchResult, now: DateTime<Utc>) -> StartupEntry {
    let label = match (hit.source, hit.role) {
        (MemoryHitSource::Message, Some(role)) => role.to_string(),
        (MemoryHitSource::Message, None) => "message".to_owned(),
        (MemoryHitSource::Summary, _) => "summary".to_owned(),
    };
    let when = relative_when(now - hit.created_at);
    #[cfg_attr(
        dylint_lib = "rr_lints",
        expect(
            fallback,
            reason = "CONTRACT B1 note 24: a hit whose session key is not a canonical session id names no session"
        )
    )]
    let source = hit
        .session_id
        .as_deref()
        .and_then(|session| session.parse::<SessionId>().ok())
        .map(StartupSource::Session);
    StartupEntry {
        text: format!(
            "[{label}, {when}] {}",
            char_prefix(hit.content.trim(), RELEVANT_CAP)
        ),
        source,
    }
}

impl From<&StartupSection> for dto::StartupSection {
    fn from(section: &StartupSection) -> Self {
        Self {
            kind: section.kind,
            title: section.title.clone(),
            text: section.text.clone(),
            sources: section
                .sources
                .iter()
                .map(|source| match source {
                    StartupSource::Session(id) => dto::StartupSource::Session(id.clone()),
                    StartupSource::Wiki(slug) => dto::StartupSource::Wiki(slug.clone()),
                })
                .collect(),
        }
    }
}

impl From<dto::StartupSection> for StartupSection {
    fn from(section: dto::StartupSection) -> Self {
        Self {
            kind: section.kind,
            title: section.title,
            text: section.text,
            sources: section
                .sources
                .into_iter()
                .map(|source| match source {
                    dto::StartupSource::Session(id) => StartupSource::Session(id),
                    dto::StartupSource::Wiki(slug) => StartupSource::Wiki(slug),
                })
                .collect(),
        }
    }
}

impl From<&StartupContext> for dto::StartupContext {
    fn from(context: &StartupContext) -> Self {
        Self {
            topic_id: context.topic.to_string(),
            sections: context.sections.iter().map(Into::into).collect(),
            tokens: context.tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str) -> StartupEntry {
        StartupEntry {
            text: text.to_owned(),
            source: None,
        }
    }

    fn part(title: &str, texts: &[&str]) -> StartupPart {
        StartupPart {
            title: title.to_owned(),
            entries: texts.iter().map(|text| entry(text)).collect(),
        }
    }

    fn parts(page: StartupPart, recent: StartupPart, relevant: StartupPart) -> StartupParts {
        StartupParts {
            topic: TopicId::random(),
            page,
            recent,
            relevant,
        }
    }

    fn cost(section: &StartupSection) -> usize {
        estimate_tokens(&section.title) + estimate_tokens(&section.text)
    }

    #[test]
    fn the_budget_holds_and_sections_keep_their_order() {
        let long = "word ".repeat(4_000);
        let context = assemble(
            parts(
                part("Mesh", &[&long]),
                part("Recent in Mesh", &[&long, &long]),
                part("Relevant memory", &[&long]),
            ),
            STARTUP_BUDGET,
        );
        let kinds: Vec<StartupKind> = context
            .sections
            .iter()
            .map(|section| section.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                StartupKind::Topic,
                StartupKind::Recent,
                StartupKind::Relevant
            ]
        );
        let spent: usize = context.sections.iter().map(cost).sum();
        assert_eq!(context.tokens, spent);
        assert!(context.tokens <= STARTUP_BUDGET);
        let [topic, recent, _] = context.sections.as_slice() else {
            panic!("three sections expected");
        };
        assert!(cost(topic) <= 1_600 && cost(topic) + cost(recent) <= 2_600);
        let small = assemble(
            parts(
                part("Mesh", &[&long]),
                part("Recent", &[&long]),
                part("Relevant", &[&long]),
            ),
            400,
        );
        assert!(small.tokens <= 400, "{}", small.tokens);
    }

    #[test]
    fn unused_budget_rolls_forward() {
        let long = "word ".repeat(4_000);
        let context = assemble(
            parts(
                part("Mesh", &["the mesh runs step-ca"]),
                part("Recent in Mesh", &[&long]),
                StartupPart::default(),
            ),
            STARTUP_BUDGET,
        );
        let [_, recent] = context.sections.as_slice() else {
            panic!("two sections expected");
        };
        assert_eq!(recent.kind, StartupKind::Recent);
        assert!(cost(recent) > 2_000, "recent took {}", cost(recent));
        assert!(cost(recent) <= 2_600);
    }

    #[test]
    fn empty_parts_give_no_sections() {
        let context = assemble(
            parts(
                part("Mesh", &["", "   "]),
                part("Recent in Mesh", &[]),
                StartupPart::default(),
            ),
            STARTUP_BUDGET,
        );
        assert_eq!(context.sections, []);
        assert_eq!(context.tokens, 0);
    }
}
