use std::collections::BTreeSet;

use chrono::DateTime;
use chrono::Utc;

use super::Alias;
use super::MAX_DEPTH;
use super::Redirect;
use super::Topic;
use super::TopicError;
use super::TopicId;
use super::TopicTree;

pub fn check_parent(child: TopicId, parent: &Topic, tree: &TopicTree) -> Result<(), TopicError> {
    if parent.id == child {
        return Err(TopicError::OwnParent);
    }
    if tree.within(parent.id, child) {
        return Err(TopicError::IntoOwnSubtree);
    }
    if tree.depth(parent.id) + tree.height(child) > MAX_DEPTH {
        return Err(TopicError::TooDeep { max: MAX_DEPTH });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergePlan {
    pub source: TopicId,
    pub into: TopicId,
    pub redirect: Redirect,
    pub aliases: Vec<Alias>,
    pub subtopics: Vec<TopicId>,
    pub into_parent: Option<TopicId>,
}

impl MergePlan {
    pub fn new(source: &Topic, into: &Topic, tree: &TopicTree) -> Result<Self, TopicError> {
        if source.id == into.id {
            return Err(TopicError::SelfMerge);
        }
        let direct = into.parent == Some(source.id);
        if !direct && tree.within(into.id, source.id) {
            return Err(TopicError::IntoOwnSubtree);
        }
        let into_parent = if direct { source.parent } else { into.parent };
        let moved: Vec<TopicId> = tree
            .children(source.id)
            .into_iter()
            .filter(|subtopic| *subtopic != into.id)
            .collect();
        let after = moved.iter().fold(
            tree.clone().without(source.id).moved(into.id, into_parent),
            |after, subtopic| after.moved(*subtopic, Some(into.id)),
        );
        if after.depth(into.id) + after.height(into.id) > MAX_DEPTH + 1 {
            return Err(TopicError::TooDeep { max: MAX_DEPTH });
        }
        let taken: BTreeSet<String> = into.aliases.iter().map(Alias::key).collect();
        let aliases = source
            .aliases
            .iter()
            .filter(|alias| !taken.contains(&alias.key()))
            .cloned()
            .collect();
        Ok(Self {
            source: source.id,
            into: into.id,
            redirect: Redirect {
                slug: source.slug.clone(),
                topic: into.id,
            },
            aliases,
            subtopics: moved,
            into_parent,
        })
    }

    #[must_use]
    pub fn merged(&self, mut into: Topic, now: DateTime<Utc>) -> Topic {
        into.aliases.extend(self.aliases.iter().cloned());
        into.aliases = super::distinct_aliases(into.aliases);
        into.parent = self.into_parent;
        into.updated_at = now;
        into
    }
}
