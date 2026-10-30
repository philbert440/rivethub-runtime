use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

use super::JunkReason;
use super::JunkStage;
use super::NameGuard;
use super::Origin;
use super::ProposedTree;
use super::TreeTopic;
use crate::topics::MAX_DEPTH;
use crate::topics::TopicId;
use crate::topics::TopicName;
use crate::topics::fold;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(tag = "change", rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum TreeChange {
    Merge {
        topic: TopicId,
        into: TopicId,
    },
    Move {
        topic: TopicId,
        under: Option<TopicId>,
    },
    Rename {
        topic: TopicId,
        name: String,
    },
    Group {
        name: String,
        about: Option<String>,
        under: Option<TopicId>,
        topics: Vec<TopicId>,
    },
}

impl ProposedTree {
    pub fn review(&mut self, changes: Vec<TreeChange>, guard: &NameGuard) {
        for change in changes {
            let applied = match &change {
                TreeChange::Merge { topic, into } => self.review_merge(*topic, *into, guard),
                TreeChange::Move { topic, under } => self.review_move(*topic, *under),
                TreeChange::Rename { topic, name } => self.review_rename(*topic, name, guard),
                TreeChange::Group {
                    name,
                    about,
                    under,
                    topics,
                } => self.review_group(name, about.as_deref(), *under, topics, guard),
            };
            if let Err(reason) = applied {
                self.note(&describe(&change), JunkStage::Review, reason);
            }
        }
        let estimates: BTreeMap<TopicId, u32> = self
            .topics
            .iter()
            .map(|topic| (topic.id, topic.estimate))
            .collect();
        self.prune(&estimates);
    }

    fn movable(&self, id: TopicId) -> Result<&TreeTopic, JunkReason> {
        match self.topic(id) {
            Some(topic) if topic.origin != Origin::Project => Ok(topic),
            Some(_) => Err(declined("a project stays a root")),
            None => Err(declined("an unknown topic")),
        }
    }

    fn review_merge(
        &mut self,
        from: TopicId,
        into: TopicId,
        guard: &NameGuard,
    ) -> Result<(), JunkReason> {
        let source = self.movable(from)?.clone();
        let target = self.topic(into).map(|topic| topic.name.clone());
        if target.is_some_and(|target| {
            guard
                .distinct(&source.name)
                .is_disjoint(&guard.distinct(&target))
        }) {
            return Err(declined(
                "a merge of topics that share no word; move or group them instead",
            ));
        }
        if from == into || self.topic(into).is_none() || self.within(into, from) {
            return Err(declined("a merge into itself or its subtopic"));
        }
        if self
            .topic(into)
            .is_some_and(|target| target.origin == Origin::Project)
        {
            return Err(declined(
                "a topic merges into a topic, not into a whole project",
            ));
        }
        if self.chain(into).len() + self.height(from) > MAX_DEPTH + 1 {
            return Err(declined("the merge passes four levels"));
        }
        if source.origin == Origin::Existing {
            self.merge_into(from, into);
        } else {
            self.topics.retain(|topic| topic.id != from);
            for topic in &mut self.topics {
                if topic.parent == Some(from) {
                    topic.parent = Some(into);
                }
            }
            if let Some(target) = self.topics.iter_mut().find(|topic| topic.id == into) {
                target.estimate = target.estimate.saturating_add(source.estimate);
            }
        }
        let into = self.path(into);
        self.note(&source.name, JunkStage::Review, JunkReason::Merged { into });
        Ok(())
    }

    fn review_move(&mut self, id: TopicId, under: Option<TopicId>) -> Result<(), JunkReason> {
        let topic = self.movable(id)?.clone();
        if let Some(parent) = under {
            if self.topic(parent).is_none() || self.within(parent, id) {
                return Err(declined("a move under itself or its subtopic"));
            }
            if self.chain(parent).len() + self.height(id) > MAX_DEPTH {
                return Err(declined("the move passes four levels"));
            }
        }
        if let Some(moved) = self.topics.iter_mut().find(|topic| topic.id == id) {
            moved.parent = under;
        }
        let under = under.map_or_else(|| "the root".to_owned(), |parent| self.path(parent));
        self.note(
            &topic.name,
            JunkStage::Review,
            JunkReason::Regrouped { under },
        );
        Ok(())
    }

    fn review_rename(
        &mut self,
        id: TopicId,
        name: &str,
        guard: &NameGuard,
    ) -> Result<(), JunkReason> {
        let topic = self.movable(id)?.clone();
        if topic.origin == Origin::Existing {
            return Err(declined("an existing topic keeps its name"));
        }
        if fold(name) == fold(&topic.name) {
            return Ok(());
        }
        self.fresh(name, Some(id), guard)?;
        if let Some(renamed) = self.topics.iter_mut().find(|topic| topic.id == id) {
            name.trim().clone_into(&mut renamed.name);
        }
        self.note(
            &topic.name,
            JunkStage::Review,
            JunkReason::Renamed {
                to: name.trim().to_owned(),
            },
        );
        Ok(())
    }

    fn review_group(
        &mut self,
        name: &str,
        about: Option<&str>,
        under: Option<TopicId>,
        members: &[TopicId],
        guard: &NameGuard,
    ) -> Result<(), JunkReason> {
        let parent = self.named(name);
        let members: Vec<TopicId> = members
            .iter()
            .copied()
            .filter(|member| {
                self.movable(*member).is_ok() && Some(*member) != under && Some(*member) != parent
            })
            .collect();
        if let Some(parent) = parent {
            return self.gather_under(parent, &members);
        }
        if members.len() < 2 {
            return Err(declined("a group needs two topics"));
        }
        if under.is_some_and(|parent| {
            self.topic(parent).is_none()
                || members.iter().any(|member| self.within(parent, *member))
        }) {
            return Err(declined("a group under one of its own topics"));
        }
        let deepest = members
            .iter()
            .map(|member| self.height(*member))
            .max()
            .unwrap_or(1);
        let above = under.map_or(0, |parent| self.chain(parent).len());
        if above + 1 + deepest > MAX_DEPTH {
            return Err(declined("the group passes four levels"));
        }
        self.fresh(name, None, guard)?;
        let group = TopicId::random();
        self.topics.push(TreeTopic {
            id: group,
            name: name.trim().to_owned(),
            parent: under,
            aliases: Vec::new(),
            about: about.map(str::to_owned),
            origin: Origin::Proposed,
            estimate: 0,
            directory: None,
        });
        self.gather_under(group, &members)
    }

    fn gather_under(&mut self, parent: TopicId, members: &[TopicId]) -> Result<(), JunkReason> {
        if members.is_empty() {
            return Err(declined("a group needs two topics"));
        }
        let deepest = members
            .iter()
            .map(|member| self.height(*member))
            .max()
            .unwrap_or(1);
        if members.iter().any(|member| self.within(parent, *member))
            || self.chain(parent).len() + deepest > MAX_DEPTH
        {
            return Err(declined("the group passes four levels or loops"));
        }
        let path = self.path(parent);
        for member in members {
            if let Some(topic) = self.topics.iter_mut().find(|topic| topic.id == *member) {
                topic.parent = Some(parent);
            }
            if let Some(name) = self.topic(*member).map(|topic| topic.name.clone()) {
                self.note(
                    &name,
                    JunkStage::Review,
                    JunkReason::Regrouped {
                        under: path.clone(),
                    },
                );
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn clusters(&self, guard: &NameGuard) -> BTreeMap<String, Vec<TopicId>> {
        let mut by_word: BTreeMap<String, Vec<TopicId>> = BTreeMap::new();
        for topic in &self.topics {
            for word in guard.distinct(&topic.name) {
                by_word.entry(word).or_default().push(topic.id);
            }
        }
        by_word.retain(|_, ids| {
            ids.iter().any(|id| {
                ids.iter().any(|other| {
                    other != id && !self.within(*other, *id) && !self.within(*id, *other)
                })
            })
        });
        by_word
    }

    fn fresh(
        &self,
        name: &str,
        except: Option<TopicId>,
        guard: &NameGuard,
    ) -> Result<(), JunkReason> {
        let topic = TopicName::new(name).map_err(|_| declined("an empty name"))?;
        if let Some(reason) = guard.junk(&topic) {
            return Err(reason);
        }
        let wanted = fold(name);
        let taken = self.topics.iter().any(|other| {
            Some(other.id) != except
                && (fold(&other.name) == wanted
                    || other.aliases.iter().any(|alias| fold(alias) == wanted))
        });
        if taken {
            return Err(declined("a name another topic has"));
        }
        Ok(())
    }

    fn height(&self, id: TopicId) -> usize {
        let below = self
            .topics
            .iter()
            .filter(|topic| topic.parent == Some(id))
            .map(|topic| self.height(topic.id))
            .max()
            .unwrap_or_default();
        below + 1
    }
}

fn declined(change: &str) -> JunkReason {
    JunkReason::Declined {
        change: change.to_owned(),
    }
}

fn describe(change: &TreeChange) -> String {
    match change {
        TreeChange::Rename { name, .. } | TreeChange::Group { name, .. } => {
            format!("{change} {name}")
        }
        TreeChange::Merge { .. } | TreeChange::Move { .. } => change.to_string(),
    }
}
