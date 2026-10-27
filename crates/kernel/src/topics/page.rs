use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;
use wire::JsonObject;
use wire::gateway::wiki::WikiPageResponse;
use wire::gateway::wiki::WikiSlug;
use wire::wiki::FoldMark;
use wire::wiki::normalize_slug;

use super::Slug;
use super::Topic;
use super::TopicError;
use super::TopicId;
use super::TopicKey;
use super::TopicStore;
use crate::BoxError;
use crate::memory::MemoryStore;
use crate::memory::MemoryStoreError;
use crate::queue::FixedJob;
use crate::queue::Job;
use crate::queue::JobKeyMode;
use crate::queue::JobName;
use crate::transcript::TranscriptReaderError;
use crate::wiki::WikiLookup;
use crate::wiki::builder::WikiStoreError;
use crate::wiki::builder::WikiWriteError;

const SLUG_ATTEMPTS: u32 = 1000;
const UNNAMED_PAGE: &str = "topic";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSection {
    pub topic: Option<TopicId>,
    pub heading: String,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicPage {
    pub slug: WikiSlug,
    pub title: String,
    pub summary: String,
    pub article: String,
    pub sections: Vec<PageSection>,
}

impl TopicPage {
    #[must_use]
    pub fn empty(slug: WikiSlug, title: &str) -> Self {
        Self {
            slug,
            title: title.to_owned(),
            summary: String::new(),
            article: String::new(),
            sections: Vec::new(),
        }
    }

    fn text(&self) -> String {
        let mut text = String::new();
        append(&mut text, &self.summary);
        append(&mut text, &self.article);
        text
    }

    pub(crate) fn add_to_section(&mut self, topic: TopicId, heading: &str, body: &str) {
        if let Some(section) = self
            .sections
            .iter_mut()
            .find(|section| section.topic == Some(topic))
        {
            append(&mut section.body, body);
            return;
        }
        let mut section = PageSection {
            topic: Some(topic),
            heading: heading.to_owned(),
            body: String::new(),
        };
        append(&mut section.body, body);
        self.sections.push(section);
    }
}

#[async_trait]
pub trait TopicPageStore: Send + Sync {
    async fn topic_page(&self, slug: &WikiSlug) -> Result<Option<TopicPage>, WikiStoreError>;

    async fn page_taken(&self, slug: &WikiSlug) -> Result<bool, WikiStoreError>;

    async fn save_topic_page(
        &self,
        topic: TopicId,
        page: &TopicPage,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError>;

    async fn redirect_page(
        &self,
        from: &WikiSlug,
        to: &WikiSlug,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError>;

    async fn fold_page(
        &self,
        child: &WikiSlug,
        into: &WikiSlug,
        mark: FoldMark,
        now: DateTime<Utc>,
    ) -> Result<(), WikiStoreError>;
}

#[derive(Debug, Error)]
pub enum TopicPageError {
    #[error(transparent)]
    Topic(#[from] TopicError),
    #[error(transparent)]
    Wiki(#[from] WikiStoreError),
    #[error(transparent)]
    Memory(#[from] MemoryStoreError),
    #[error(transparent)]
    Lineage(#[from] TranscriptReaderError),
    #[error("sync-topic-page payload: {0}")]
    Payload(#[source] BoxError),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncTopicPage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic_id: Option<TopicId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fold: Vec<WikiSlug>,
}

impl SyncTopicPage {
    #[must_use]
    pub fn topic(id: TopicId) -> Self {
        Self {
            topic_id: Some(id),
            fold: Vec::new(),
        }
    }

    #[must_use]
    pub fn every_topic() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn after_create(topic: &Topic) -> Vec<Self> {
        vec![Self::topic(topic.id)]
    }

    #[must_use]
    pub fn after_update(before: &Topic, after: &Topic) -> Vec<Self> {
        let mut syncs = vec![Self::topic(after.id)];
        if before.root() != after.root() && before.root() != after.id {
            syncs.push(Self::topic(before.root()));
        }
        syncs
    }

    #[must_use]
    pub fn after_merge(source: &Topic, into: &Topic, page: Option<WikiSlug>) -> Vec<Self> {
        let mut syncs = vec![Self {
            topic_id: Some(into.id),
            fold: page.into_iter().collect(),
        }];
        if source.root() != source.id && source.root() != into.root() {
            syncs.push(Self::topic(source.root()));
        }
        syncs
    }

    pub fn job(&self) -> Result<Job, TopicPageError> {
        let key = match (self.topic_id, self.fold.is_empty()) {
            (Some(id), true) => Some(format!("sync-topic-page:{id}")),
            (Some(_), false) => None,
            (None, _) => Some("sync-topic-page:all".to_owned()),
        };
        Ok(Job {
            name: JobName::Fixed(FixedJob::SyncTopicPage),
            payload: JsonObject::from_serialize(self)
                .map_err(|source| TopicPageError::Payload(Box::new(source)))?,
            key,
            key_mode: JobKeyMode::Replace,
            priority: 0,
            run_at: None,
            max_attempts: None,
        })
    }

    pub fn from_payload(payload: &JsonObject) -> Result<Self, TopicPageError> {
        payload
            .deserialize()
            .map_err(|source| TopicPageError::Payload(Box::new(source)))
    }
}

pub async fn bound_page(
    topics: &dyn TopicStore,
    memory: &dyn MemoryStore,
    topic: &Topic,
) -> Result<Option<WikiPageResponse>, TopicPageError> {
    let Some(slug) = topics.page_slug(topic.root()).await? else {
        return Ok(None);
    };
    Ok(match memory.wiki_page(&slug).await? {
        WikiLookup::Page(page) => Some(*page),
        WikiLookup::Redirect(_) | WikiLookup::Missing => None,
    })
}

pub(crate) async fn root_page(
    topics: &dyn TopicStore,
    pages: &dyn TopicPageStore,
    root: &Topic,
    now: DateTime<Utc>,
) -> Result<WikiSlug, TopicPageError> {
    PageSync { topics, pages, now }.ensure(root).await
}

pub async fn sync_topic_page(
    topics: &dyn TopicStore,
    pages: &dyn TopicPageStore,
    job: &SyncTopicPage,
    now: DateTime<Utc>,
) -> Result<(), TopicPageError> {
    let sync = PageSync { topics, pages, now };
    if let Some(id) = job.topic_id {
        return sync.topic(id, &job.fold).await;
    }
    for listing in topics.list().await? {
        let topic = listing.topic;
        if topic.parent.is_none() || topics.page_slug(topic.id).await?.is_some() {
            sync.topic(topic.id, &[]).await?;
        }
    }
    Ok(())
}

enum Landing {
    Article,
    Section(TopicId),
    Elsewhere(Topic),
    Plain,
}

struct PageSync<'a> {
    topics: &'a dyn TopicStore,
    pages: &'a dyn TopicPageStore,
    now: DateTime<Utc>,
}

impl PageSync<'_> {
    async fn topic(&self, id: TopicId, fold: &[WikiSlug]) -> Result<(), TopicPageError> {
        let Some(topic) = self.topics.find(&TopicKey::Id(id)).await? else {
            return Ok(());
        };
        let root = self.owner(&topic).await?;
        let home = self.ensure(&root).await?;
        if topic.parent.is_some()
            && let Some(own) = self.topics.page_slug(topic.id).await?
            && own != home
        {
            self.absorb(&root, &topic, &own, &home).await?;
        }
        for child in fold.iter().filter(|child| **child != home) {
            self.pages
                .fold_page(child, &home, FoldMark::Merged, self.now)
                .await?;
        }
        self.reconcile(&root, &home).await
    }

    async fn owner(&self, topic: &Topic) -> Result<Topic, TopicError> {
        match topic.parent {
            None => Ok(topic.clone()),
            Some(parent) => self
                .topics
                .find(&TopicKey::Id(parent))
                .await?
                .ok_or_else(|| TopicError::NotFound {
                    key: parent.to_string(),
                }),
        }
    }

    async fn ensure(&self, root: &Topic) -> Result<WikiSlug, TopicPageError> {
        if let Some(slug) = self.topics.page_slug(root.id).await? {
            return Ok(slug);
        }
        let slug = self.free_slug(&root.slug).await?;
        let page = TopicPage::empty(slug.clone(), root.name.as_str());
        self.pages.save_topic_page(root.id, &page, self.now).await?;
        Ok(slug)
    }

    async fn free_slug(&self, topic: &Slug) -> Result<WikiSlug, TopicPageError> {
        let base = page_slug_text(topic);
        for attempt in 1..=SLUG_ATTEMPTS {
            let text = if attempt == 1 {
                base.clone()
            } else {
                format!("{base}-{attempt}")
            };
            let slug = wiki_slug(text)?;
            if !self.pages.page_taken(&slug).await? {
                return Ok(slug);
            }
        }
        Err(WikiStoreError::SlugConflict(base).into())
    }

    async fn read(&self, slug: &WikiSlug) -> Result<TopicPage, TopicPageError> {
        self.pages
            .topic_page(slug)
            .await?
            .ok_or_else(|| WikiStoreError::TopicMissing(slug.as_ref().to_owned()).into())
    }

    async fn save(
        &self,
        owner: TopicId,
        before: &TopicPage,
        after: &TopicPage,
    ) -> Result<(), TopicPageError> {
        if before != after {
            self.pages.save_topic_page(owner, after, self.now).await?;
        }
        Ok(())
    }

    async fn absorb(
        &self,
        root: &Topic,
        topic: &Topic,
        own: &WikiSlug,
        home: &WikiSlug,
    ) -> Result<(), TopicPageError> {
        let Some(child) = self.pages.topic_page(own).await? else {
            return Ok(());
        };
        let text = child.text();
        if !text.is_empty() {
            let before = self.read(home).await?;
            let mut after = before.clone();
            after.add_to_section(topic.id, topic.name.as_str(), &text);
            self.save(root.id, &before, &after).await?;
            let mut emptied = child.clone();
            emptied.summary.clear();
            emptied.article.clear();
            self.save(topic.id, &child, &emptied).await?;
        }
        self.pages
            .fold_page(own, home, FoldMark::Section, self.now)
            .await?;
        Ok(())
    }

    async fn reconcile(&self, root: &Topic, home: &WikiSlug) -> Result<(), TopicPageError> {
        let before = self.read(home).await?;
        let subtopics = self.subtopics(root.id).await?;
        let mut owned: Vec<PageSection> = Vec::new();
        let mut plain: Vec<PageSection> = Vec::new();
        let mut into_article: Vec<String> = Vec::new();
        let mut into_sections: Vec<(TopicId, String)> = Vec::new();
        let mut moved: Vec<(Topic, String)> = Vec::new();
        for section in &before.sections {
            match section.topic {
                None => plain.push(section.clone()),
                Some(id) if subtopics.iter().any(|sub| sub.id == id) => {
                    match owned.iter_mut().find(|kept| kept.topic == Some(id)) {
                        Some(kept) => append(&mut kept.body, &section.body),
                        None => owned.push(section.clone()),
                    }
                }
                Some(_) => match self.landing(root, section).await? {
                    Landing::Article => into_article.push(section.body.clone()),
                    Landing::Section(id) => into_sections.push((id, section.body.clone())),
                    Landing::Elsewhere(target) => moved.push((target, section.body.clone())),
                    Landing::Plain => plain.push(PageSection {
                        topic: None,
                        ..section.clone()
                    }),
                },
            }
        }
        let mut after = before.clone();
        root.name.as_str().clone_into(&mut after.title);
        after.sections = arranged(&subtopics, owned, &mut plain, &into_sections);
        after.sections.extend(plain);
        for body in &into_article {
            append(&mut after.article, body);
        }
        for (target, body) in &moved {
            self.hand_off(target, body).await?;
        }
        self.save(root.id, &before, &after).await?;
        let topic_slug = wiki_slug(page_slug_text(&root.slug))?;
        if topic_slug != *home {
            self.pages
                .redirect_page(&topic_slug, home, self.now)
                .await?;
        }
        Ok(())
    }

    async fn subtopics(&self, root: TopicId) -> Result<Vec<Topic>, TopicError> {
        let mut subtopics: Vec<Topic> = self
            .topics
            .list()
            .await?
            .into_iter()
            .map(|listing| listing.topic)
            .filter(|topic| topic.parent == Some(root))
            .collect();
        subtopics.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.slug.cmp(&right.slug))
        });
        Ok(subtopics)
    }

    async fn landing(&self, root: &Topic, section: &PageSection) -> Result<Landing, TopicError> {
        let found = match section.topic {
            Some(id) => self.topics.find(&TopicKey::Id(id)).await?,
            None => None,
        };
        let target = match found {
            Some(topic) => Some(topic),
            None => match Slug::of_heading(&section.heading) {
                Some(slug) => self.topics.find(&TopicKey::Slug(slug)).await?,
                None => None,
            },
        };
        Ok(match target {
            None => Landing::Plain,
            Some(topic) if topic.id == root.id => Landing::Article,
            Some(topic) if topic.parent == Some(root.id) => Landing::Section(topic.id),
            Some(topic) => Landing::Elsewhere(topic),
        })
    }

    async fn hand_off(&self, target: &Topic, body: &str) -> Result<(), TopicPageError> {
        if body.trim().is_empty() {
            return Ok(());
        }
        let owner = self.owner(target).await?;
        let slug = self.ensure(&owner).await?;
        let before = self.read(&slug).await?;
        let mut after = before.clone();
        if target.parent.is_some() {
            after.add_to_section(target.id, target.name.as_str(), body);
        } else {
            append(&mut after.article, body);
        }
        self.save(owner.id, &before, &after).await
    }
}

fn arranged(
    subtopics: &[Topic],
    mut owned: Vec<PageSection>,
    plain: &mut Vec<PageSection>,
    additions: &[(TopicId, String)],
) -> Vec<PageSection> {
    subtopics
        .iter()
        .map(|sub| {
            let mut section = match owned.iter().position(|kept| kept.topic == Some(sub.id)) {
                Some(index) => owned.remove(index),
                None => match plain
                    .iter()
                    .position(|loose| loose.heading.eq_ignore_ascii_case(sub.name.as_str()))
                {
                    Some(index) => plain.remove(index),
                    None => PageSection {
                        topic: None,
                        heading: String::new(),
                        body: String::new(),
                    },
                },
            };
            section.topic = Some(sub.id);
            sub.name.as_str().clone_into(&mut section.heading);
            for (_, body) in additions.iter().filter(|(id, _)| *id == sub.id) {
                append(&mut section.body, body);
            }
            section
        })
        .collect()
}

fn append(target: &mut String, text: &str) {
    let text = text.trim();
    if text.is_empty() || target.contains(text) {
        return;
    }
    *target = if target.trim().is_empty() {
        text.to_owned()
    } else {
        format!("{}\n\n{text}", target.trim_end())
    };
}

fn page_slug_text(topic: &Slug) -> String {
    let slug = normalize_slug(topic.as_str());
    if slug.is_empty() {
        UNNAMED_PAGE.to_owned()
    } else {
        slug
    }
}

fn wiki_slug(text: String) -> Result<WikiSlug, WikiStoreError> {
    WikiSlug::try_from(text).map_err(|error| WikiStoreError::Page(WikiWriteError::Slug(error)))
}
