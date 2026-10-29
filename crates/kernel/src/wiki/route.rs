use std::str::FromStr;

use chrono::DateTime;
use chrono::Utc;
use serde::Serialize;
use wire::gateway::wiki::WikiSlug;
use wire::session_id::SessionId;
use wire::wiki::demote_h2_headings;

use super::builder::EntityKind;
use super::builder::ExtractWiki;
use super::builder::WikiBuilderStore;
use super::builder::WikiPatch;
use super::builder::WikiRouting;
use super::builder::WikiStoreError;
use super::builder::WikiSummary;
use crate::BoxError;
use crate::topics::NewTopic;
use crate::topics::Placement;
use crate::topics::Slug;
use crate::topics::Topic;
use crate::topics::TopicError;
use crate::topics::TopicId;
use crate::topics::TopicKey;
use crate::topics::TopicName;
use crate::topics::TopicPageError;
use crate::topics::TopicPageStore;
use crate::topics::TopicStore;
use crate::topics::root_of;
use crate::topics::root_page;
use crate::transcript::SessionLister;
use crate::transcript::TranscriptReaderError;
use crate::transcript::linked_parent;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TopicTarget {
    #[serde(skip)]
    pub topic: TopicId,
    #[serde(skip)]
    pub root: TopicId,
    pub page_slug: WikiSlug,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
}

impl TopicTarget {
    fn claims(&self, patch: &WikiPatch) -> bool {
        patch.slug == self.page_slug || patch.aliases.contains(&self.page_slug)
    }

    fn page_patch(&self, mut patch: WikiPatch) -> WikiPatch {
        patch.slug = self.page_slug.clone();
        patch.aliases.retain(|alias| *alias != self.page_slug);
        self.name.clone_into(&mut patch.title);
        patch
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::Display, strum::EnumIter)]
pub enum Category {
    People,
    Hosts,
    Tools,
}

impl From<EntityKind> for Category {
    fn from(kind: EntityKind) -> Self {
        match kind {
            EntityKind::Person => Self::People,
            EntityKind::Host => Self::Hosts,
            EntityKind::Tool => Self::Tools,
        }
    }
}

#[expect(
    missing_debug_implementations,
    reason = "kernel ports are trait objects"
)]
#[derive(Clone, Copy)]
pub struct TopicPorts<'a> {
    pub topics: &'a dyn TopicStore,
    pub pages: &'a dyn TopicPageStore,
    pub lineage: &'a dyn SessionLister,
}

#[expect(
    missing_debug_implementations,
    reason = "kernel ports are trait objects"
)]
#[derive(Clone, Copy)]
pub struct WikiRouter<'a> {
    pub wiki: &'a dyn WikiBuilderStore,
    pub topics: Option<TopicPorts<'a>>,
    pub now: DateTime<Utc>,
}

impl WikiRouter<'_> {
    pub async fn target(
        &self,
        summary: &WikiSummary,
    ) -> Result<Option<TopicTarget>, TopicPageError> {
        let (Some(ports), Some(session)) = (self.topics, session(summary)) else {
            return Ok(None);
        };
        let placement = match ports.topics.placement(&session).await? {
            Some(placement) => Some(placement),
            None => ports
                .parent_placement(&session)
                .await
                .map_err(TopicPageError::ParentPlacement)?,
        };
        let Some(placement) = placement else {
            return Ok(None);
        };
        match ports.topics.find(&TopicKey::Id(placement.topic)).await? {
            Some(topic) => ports.target(&topic, self.now).await.map(Some),
            None => Ok(None),
        }
    }

    pub async fn apply(
        &self,
        job: &ExtractWiki,
        target: Option<&TopicTarget>,
        patches: Vec<WikiPatch>,
    ) -> Result<Vec<String>, TopicPageError> {
        let Some(ports) = self.topics else {
            return Ok(self
                .wiki
                .apply_wiki(job, patches, &WikiRouting::default(), self.now)
                .await?);
        };
        let mut kept = Vec::new();
        let mut cited: Vec<WikiSlug> = target
            .map(|topic| topic.page_slug.clone())
            .into_iter()
            .collect();
        let mut written = Vec::new();
        for patch in patches {
            let route = match target.filter(|topic| topic.claims(&patch)) {
                Some(topic) => Some(topic.clone()),
                None => self.entity(ports, &patch).await?,
            };
            match route {
                None => kept.push(patch),
                Some(route) if route.section.is_none() => kept.push(route.page_patch(patch)),
                Some(route) => {
                    self.write_section(ports, &route, &patch).await?;
                    written.push(route.page_slug.as_ref().to_owned());
                    cited.push(route.page_slug);
                }
            }
        }
        cited.sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
        cited.dedup();
        let routing = WikiRouting {
            topic_page: target.map(|topic| topic.page_slug.clone()),
            cited,
        };
        let mut touched = self.wiki.apply_wiki(job, kept, &routing, self.now).await?;
        touched.extend(written);
        touched.sort();
        touched.dedup();
        Ok(touched)
    }

    async fn entity(
        &self,
        ports: TopicPorts<'_>,
        patch: &WikiPatch,
    ) -> Result<Option<TopicTarget>, TopicPageError> {
        let Some(kind) = patch.kind else {
            return Ok(None);
        };
        let Some(topic) = ports.filed(kind, patch, self.now).await? else {
            return Ok(None);
        };
        ports.target(&topic, self.now).await.map(Some)
    }

    async fn write_section(
        &self,
        ports: TopicPorts<'_>,
        route: &TopicTarget,
        patch: &WikiPatch,
    ) -> Result<(), TopicPageError> {
        let Some(heading) = &route.section else {
            return Ok(());
        };
        let before = ports
            .pages
            .topic_page(&route.page_slug)
            .await?
            .ok_or_else(|| WikiStoreError::TopicMissing(route.page_slug.as_ref().to_owned()))?;
        let mut after = before.clone();
        after.add_to_section(route.topic, heading, &section_text(patch));
        if after != before {
            ports
                .pages
                .save_topic_page(route.root, &after, self.now)
                .await?;
        }
        Ok(())
    }
}

impl TopicPorts<'_> {
    async fn parent_placement(&self, session: &SessionId) -> Result<Option<Placement>, BoxError> {
        match self.parent(session).await? {
            Some(parent) => Ok(self.topics.placement(&parent).await?),
            None => Ok(None),
        }
    }

    async fn parent(
        &self,
        session: &SessionId,
    ) -> Result<Option<SessionId>, TranscriptReaderError> {
        let described = self
            .lineage
            .describe(session.harness(), session.native(), None)
            .await?;
        Ok(described
            .and_then(|row| row.parent_session_id)
            .and_then(|parent| linked_parent(session.harness(), &parent)))
    }

    async fn target(
        &self,
        topic: &Topic,
        now: DateTime<Utc>,
    ) -> Result<TopicTarget, TopicPageError> {
        let root = root_of(self.topics, topic).await?;
        let page_slug = root_page(self.topics, self.pages, &root, now).await?;
        Ok(TopicTarget {
            topic: topic.id,
            root: root.id,
            page_slug,
            name: root.name.as_str().to_owned(),
            section: topic.parent.map(|_| topic.name.as_str().to_owned()),
        })
    }

    async fn filed(
        &self,
        kind: EntityKind,
        patch: &WikiPatch,
        now: DateTime<Utc>,
    ) -> Result<Option<Topic>, TopicError> {
        if let Some(topic) = self.known(patch).await? {
            return Ok(Some(topic));
        }
        let Some(category) = self.category(kind, now).await? else {
            return Ok(None);
        };
        let entity = NewTopic {
            name: TopicName::new(&patch.title)?,
            aliases: Vec::new(),
            parent: Some(category.id),
            directory: None,
        };
        let created = self
            .topics
            .create(Topic::create(TopicId::random(), entity, now)?)
            .await?;
        Ok(Some(created))
    }

    async fn known(&self, patch: &WikiPatch) -> Result<Option<Topic>, TopicError> {
        let names = [patch.title.as_str(), patch.slug.as_ref()]
            .into_iter()
            .chain(patch.aliases.iter().map(AsRef::as_ref));
        for name in names {
            if let Some(slug) = Slug::of_heading(name)
                && let Some(topic) = self.topics.find(&TopicKey::Slug(slug)).await?
            {
                return Ok(Some(topic));
            }
        }
        Ok(None)
    }

    async fn category(
        &self,
        kind: EntityKind,
        now: DateTime<Utc>,
    ) -> Result<Option<Topic>, TopicError> {
        let name = TopicName::new(&Category::from(kind).to_string())?;
        match self
            .topics
            .find(&TopicKey::Slug(Slug::fold(name.as_str())?))
            .await?
        {
            Some(topic) if topic.parent.is_none() => Ok(Some(topic)),
            Some(_) => Ok(None),
            None => {
                let root = NewTopic {
                    name,
                    aliases: Vec::new(),
                    parent: None,
                    directory: None,
                };
                let created = self
                    .topics
                    .create(Topic::create(TopicId::random(), root, now)?)
                    .await?;
                Ok(Some(created))
            }
        }
    }
}

fn session(summary: &WikiSummary) -> Option<SessionId> {
    let key = summary.session_key.as_deref()?;
    #[cfg_attr(
        dylint_lib = "rr_lints",
        expect(
            fallback,
            reason = "PATTERNS FALLBACK: a conversation whose session key is not a canonical session id has no topic placement"
        )
    )]
    let Ok(session) = SessionId::from_str(key) else {
        return None;
    };
    Some(session)
}

fn section_text(patch: &WikiPatch) -> String {
    let summary = demote_h2_headings(patch.summary.trim());
    match patch
        .article
        .as_deref()
        .map(str::trim)
        .filter(|article| !article.is_empty())
    {
        Some(article) => format!("{summary}\n\n{}", demote_h2_headings(article)),
        None => summary,
    }
}
