use std::collections::BTreeSet;

use chrono::DateTime;
use chrono::Utc;
use uuid::Uuid;

use super::builder::HeldEntity;
use super::builder::WikiBuilderStore;
use super::builder::WikiSummary;
use super::page::WikiPatch;
use crate::topics::NewTopic;
use crate::topics::Redirect;
use crate::topics::Slug;
use crate::topics::Topic;
use crate::topics::TopicId;
use crate::topics::TopicKind;
use crate::topics::TopicName;
use crate::topics::TopicPageError;
use crate::topics::TopicStore;
use crate::topics::TopicTree;

const SESSIONS_TO_CREATE: usize = 2;
const PATH_SEPARATOR: &str = " › ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filed {
    pub entity: Topic,
    pub patches: Vec<WikiPatch>,
    pub earlier: Vec<Uuid>,
    pub held: Vec<HeldEntity>,
}

impl Filed {
    fn of(entity: Topic, summary: &WikiSummary, patch: WikiPatch, held: Vec<HeldEntity>) -> Self {
        let own = source(summary);
        let others: Vec<&HeldEntity> = held
            .iter()
            .filter(|mention| mention.source != own)
            .collect();
        let mut patches: Vec<WikiPatch> =
            others.iter().map(|mention| mention.patch.clone()).collect();
        patches.push(patch);
        Self {
            entity,
            patches,
            earlier: others
                .iter()
                .map(|mention| mention.summary_id)
                .filter(|id| *id != summary.id)
                .collect(),
            held,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    entities: Vec<Topic>,
    types: Vec<Topic>,
    redirects: Vec<Redirect>,
}

impl Catalog {
    pub async fn load(topics: &dyn TopicStore) -> Result<Self, TopicPageError> {
        let mut entities = Vec::new();
        let mut types = Vec::new();
        for listing in topics.list().await? {
            match listing.topic.kind {
                TopicKind::Entity => entities.push(listing.topic),
                TopicKind::Type => types.push(listing.topic),
                TopicKind::Work => {}
            }
        }
        Ok(Self {
            entities,
            types,
            redirects: topics.redirects().await?,
        })
    }

    #[must_use]
    pub fn type_paths(&self) -> Vec<String> {
        let tree = TopicTree::of(&self.types);
        let mut paths: Vec<String> = self
            .types
            .iter()
            .map(|kind| {
                let mut names: Vec<&str> = tree
                    .ancestors(kind.id)
                    .filter_map(|above| self.types.iter().find(|known| known.id == above))
                    .map(|above| above.name.as_str())
                    .collect();
                names.reverse();
                names.push(kind.name.as_str());
                names.join(PATH_SEPARATOR)
            })
            .collect();
        paths.sort();
        paths
    }

    pub async fn file(
        &mut self,
        topics: &dyn TopicStore,
        wiki: &dyn WikiBuilderStore,
        summary: &WikiSummary,
        patch: WikiPatch,
        now: DateTime<Utc>,
    ) -> Result<Option<Filed>, TopicPageError> {
        let names = names(&patch);
        if let Some(entity) = self.known(&names).cloned() {
            let keys: BTreeSet<Slug> = names.into_iter().chain(self.keys(&entity)).collect();
            let mut held = Vec::new();
            for key in &keys {
                held.extend(wiki.held_entities(key).await?);
            }
            held.sort_by(|left, right| {
                left.at
                    .cmp(&right.at)
                    .then_with(|| left.source.cmp(&right.source))
            });
            return Ok(Some(Filed::of(entity, summary, patch, held)));
        }
        for name in &names {
            if topics.tombstoned(TopicKind::Entity, name).await? {
                return Ok(None);
            }
        }
        let Some(key) = Slug::of_heading(&patch.title) else {
            return Ok(None);
        };
        let held = wiki
            .hold_entity(HeldEntity {
                key,
                source: source(summary),
                summary_id: summary.id,
                patch: patch.clone(),
                at: now,
            })
            .await?;
        let sources: BTreeSet<&str> = held.iter().map(|mention| mention.source.as_str()).collect();
        if sources.len() < SESSIONS_TO_CREATE {
            return Ok(None);
        }
        let Some(parent) = held
            .iter()
            .rev()
            .filter_map(|mention| mention.patch.entity_type.as_deref())
            .find_map(|named| self.type_named(named))
            .map(|kind| kind.id)
        else {
            return Ok(None);
        };
        let Some(latest) = held.last() else {
            return Ok(None);
        };
        let created = topics
            .create(Topic::create(
                TopicId::random(),
                NewTopic {
                    kind: TopicKind::Entity,
                    name: TopicName::new(&latest.patch.title)?,
                    aliases: Vec::new(),
                    parent: Some(parent),
                    directory: None,
                },
                now,
            )?)
            .await?;
        self.entities.push(created.clone());
        Ok(Some(Filed::of(created, summary, patch, held)))
    }

    fn known(&self, names: &BTreeSet<Slug>) -> Option<&Topic> {
        self.entities
            .iter()
            .find(|entity| self.keys(entity).any(|key| names.contains(&key)))
    }

    fn type_named(&self, named: &str) -> Option<&Topic> {
        let last = named
            .rsplit(['›', '>', '/'])
            .map(str::trim)
            .find(|part| !part.is_empty())?;
        let key = Slug::of_heading(last)?;
        self.types
            .iter()
            .find(|kind| self.keys(kind).any(|known| known == key))
    }

    fn keys<'a>(&'a self, topic: &'a Topic) -> impl Iterator<Item = Slug> + 'a {
        std::iter::once(topic.slug.clone())
            .chain(
                topic
                    .aliases
                    .iter()
                    .filter_map(|alias| Slug::of_heading(alias.as_str())),
            )
            .chain(
                self.redirects
                    .iter()
                    .filter(|redirect| redirect.topic == topic.id)
                    .map(|redirect| redirect.slug.clone()),
            )
    }
}

fn names(patch: &WikiPatch) -> BTreeSet<Slug> {
    [patch.title.as_str(), patch.slug.as_ref()]
        .into_iter()
        .chain(patch.aliases.iter().map(AsRef::as_ref))
        .filter_map(Slug::of_heading)
        .collect()
}

fn source(summary: &WikiSummary) -> String {
    match (&summary.session_key, summary.conversation_id) {
        (Some(key), _) => key.clone(),
        (None, Some(conversation)) => conversation.to_string(),
        (None, None) => summary.id.to_string(),
    }
}
