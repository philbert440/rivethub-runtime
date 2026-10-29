use std::str::FromStr;

use chrono::DateTime;
use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;
use wire::gateway::wiki::WikiSlug;
use wire::session_id::SessionId;
use wire::wiki::demote_h2_headings;

use super::builder::DeferredWiki;
use super::builder::ExtractWiki;
use super::builder::WikiBuilderStore;
use super::builder::WikiPatch;
use super::builder::WikiRouting;
use super::builder::WikiStoreError;
use super::builder::WikiSummary;
use super::entity::Catalog;
use super::entity::Filed;
use crate::BoxError;
use crate::topics::Placement;
use crate::topics::Topic;
use crate::topics::TopicId;
use crate::topics::TopicKey;
use crate::topics::TopicPage;
use crate::topics::TopicPageError;
use crate::topics::TopicPageStore;
use crate::topics::TopicStore;
use crate::topics::page_owner;
use crate::topics::root_page;
use crate::transcript::SessionLister;
use crate::transcript::TranscriptReaderError;
use crate::transcript::linked_parent;

const DEFERRALS_PER_SWEEP: u32 = 50;

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
    pub topics: TopicPorts<'a>,
    pub now: DateTime<Utc>,
}

#[derive(Default)]
struct Routed {
    pages: Vec<WikiPatch>,
    written: Vec<WikiSlug>,
    cited: Vec<WikiSlug>,
    earlier: Vec<(Uuid, WikiSlug)>,
    waiting: Vec<WikiPatch>,
}

impl WikiRouter<'_> {
    pub async fn target(
        &self,
        summary: &WikiSummary,
    ) -> Result<Option<TopicTarget>, TopicPageError> {
        let ports = self.topics;
        let Some(session) = session(summary) else {
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
            Some(topic) => ports.target(&topic, self.now).await,
            None => Ok(None),
        }
    }

    pub async fn type_paths(&self) -> Result<Vec<String>, TopicPageError> {
        Ok(Catalog::load(self.topics.topics).await?.type_paths())
    }

    pub async fn apply(
        &self,
        job: &ExtractWiki,
        summary: &WikiSummary,
        target: Option<&TopicTarget>,
        patches: Vec<WikiPatch>,
    ) -> Result<Vec<String>, TopicPageError> {
        let mut routed = Routed {
            cited: target
                .map(|topic| topic.page_slug.clone())
                .into_iter()
                .collect(),
            ..Routed::default()
        };
        let mut entities = None;
        for patch in patches {
            if let Some(topic) = target.filter(|topic| topic.claims(&patch)) {
                self.route_to_topic(topic, patch, &mut routed).await?;
            } else if patch.entity_type.is_some() {
                let known = match entities.take() {
                    Some(known) => known,
                    None => Catalog::load(self.topics.topics).await?,
                };
                entities = Some(
                    self.route_to_entity(known, summary, patch, &mut routed)
                        .await?,
                );
            } else if let Some(topic) = target {
                self.write_note(topic, &patch).await?;
                routed.written.push(topic.page_slug.clone());
            } else {
                routed.waiting.push(patch);
            }
        }
        self.finish(job, summary, target, routed).await
    }

    pub async fn apply_deferred(&self) -> Result<Vec<String>, TopicPageError> {
        let mut touched = Vec::new();
        for deferred in self.wiki.filed_deferrals(DEFERRALS_PER_SWEEP).await? {
            let summary = WikiSummary {
                id: deferred.summary_id,
                conversation_id: None,
                content: String::new(),
                session_key: Some(deferred.session_key.clone()),
            };
            let Some(topic) = self.target(&summary).await? else {
                continue;
            };
            for patch in &deferred.patches {
                self.write_note(&topic, patch).await?;
            }
            self.wiki
                .cite_wiki(deferred.summary_id, &topic.page_slug, self.now)
                .await?;
            self.wiki.forget_deferral(deferred.summary_id).await?;
            touched.push(topic.page_slug.as_ref().to_owned());
        }
        touched.sort();
        touched.dedup();
        Ok(touched)
    }

    async fn route_to_topic(
        &self,
        topic: &TopicTarget,
        patch: WikiPatch,
        routed: &mut Routed,
    ) -> Result<(), TopicPageError> {
        if topic.section.is_none() {
            routed.pages.push(topic.page_patch(patch));
            return Ok(());
        }
        self.write_section(topic, &patch).await?;
        routed.written.push(topic.page_slug.clone());
        Ok(())
    }

    async fn route_to_entity(
        &self,
        mut known: Catalog,
        summary: &WikiSummary,
        patch: WikiPatch,
        routed: &mut Routed,
    ) -> Result<Catalog, TopicPageError> {
        let filing = known
            .file(self.topics.topics, self.wiki, summary, patch, self.now)
            .await?;
        if let Some(Filed {
            entity,
            patches,
            earlier,
        }) = filing
            && let Some(page) = self.topics.target(&entity, self.now).await?
        {
            routed
                .earlier
                .extend(earlier.into_iter().map(|id| (id, page.page_slug.clone())));
            routed
                .pages
                .extend(patches.into_iter().map(|patch| page.page_patch(patch)));
        }
        Ok(known)
    }

    async fn finish(
        &self,
        job: &ExtractWiki,
        summary: &WikiSummary,
        target: Option<&TopicTarget>,
        mut routed: Routed,
    ) -> Result<Vec<String>, TopicPageError> {
        if let (false, Some(session_key)) = (routed.waiting.is_empty(), &summary.session_key) {
            self.wiki
                .defer_wiki(DeferredWiki {
                    summary_id: summary.id,
                    session_key: session_key.clone(),
                    patches: routed.waiting,
                    at: self.now,
                })
                .await?;
        }
        routed.cited.extend(routed.written.iter().cloned());
        routed
            .cited
            .sort_by(|left, right| left.as_ref().cmp(right.as_ref()));
        routed.cited.dedup();
        let routing = WikiRouting {
            topic_page: target.map(|topic| topic.page_slug.clone()),
            cited: routed.cited,
        };
        let mut touched = self
            .wiki
            .apply_wiki(job, routed.pages, &routing, self.now)
            .await?;
        for (earlier, page) in &routed.earlier {
            self.wiki.cite_wiki(*earlier, page, self.now).await?;
        }
        touched.extend(routed.written.into_iter().map(String::from));
        touched.sort();
        touched.dedup();
        Ok(touched)
    }

    async fn write_section(
        &self,
        route: &TopicTarget,
        patch: &WikiPatch,
    ) -> Result<(), TopicPageError> {
        let Some(heading) = &route.section else {
            return Ok(());
        };
        let text = section_text(patch);
        self.edit_page(route, |page| {
            page.add_to_section(route.topic, heading, &text);
        })
        .await
    }

    async fn write_note(
        &self,
        route: &TopicTarget,
        patch: &WikiPatch,
    ) -> Result<(), TopicPageError> {
        let text = section_text(patch);
        match &route.section {
            Some(heading) => {
                let body = format!("### {}\n\n{text}", patch.title.trim());
                self.edit_page(route, |page| {
                    page.add_to_section(route.topic, heading, &body);
                })
                .await
            }
            None => {
                self.edit_page(route, |page| page.add_note(patch.title.trim(), &text))
                    .await
            }
        }
    }

    async fn edit_page(
        &self,
        route: &TopicTarget,
        change: impl FnOnce(&mut TopicPage),
    ) -> Result<(), TopicPageError> {
        let pages = self.topics.pages;
        let before = pages
            .topic_page(&route.page_slug)
            .await?
            .ok_or_else(|| WikiStoreError::TopicMissing(route.page_slug.as_ref().to_owned()))?;
        let mut after = before.clone();
        change(&mut after);
        if after != before {
            pages.save_topic_page(route.root, &after, self.now).await?;
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
    ) -> Result<Option<TopicTarget>, TopicPageError> {
        let Some(owner) = page_owner(self.topics, topic).await? else {
            return Ok(None);
        };
        let page_slug = root_page(self.topics, self.pages, &owner, now).await?;
        Ok(Some(TopicTarget {
            topic: topic.id,
            root: owner.id,
            page_slug,
            name: owner.name.as_str().to_owned(),
            section: (owner.id != topic.id).then(|| topic.name.as_str().to_owned()),
        }))
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
