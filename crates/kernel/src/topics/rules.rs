use std::collections::BTreeSet;

use chrono::DateTime;
use chrono::Utc;

use super::Alias;
use super::Redirect;
use super::Topic;
use super::TopicError;
use super::TopicId;

pub fn check_parent(
    child: TopicId,
    child_has_subtopics: bool,
    parent: &Topic,
) -> Result<(), TopicError> {
    if parent.id == child {
        return Err(TopicError::OwnParent);
    }
    if parent.parent.is_some() {
        return Err(TopicError::ParentIsSubtopic {
            parent: parent.slug.clone(),
        });
    }
    if child_has_subtopics {
        return Err(TopicError::HasSubtopics);
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
    pub fn new(source: &Topic, into: &Topic, subtopics: &[TopicId]) -> Result<Self, TopicError> {
        if source.id == into.id {
            return Err(TopicError::SelfMerge);
        }
        let into_parent = if into.parent == Some(source.id) {
            source.parent
        } else {
            into.parent
        };
        let moved: Vec<TopicId> = subtopics
            .iter()
            .copied()
            .filter(|subtopic| *subtopic != into.id)
            .collect();
        if !moved.is_empty() && into_parent.is_some() {
            return Err(TopicError::MergeWouldNest {
                into: into.slug.clone(),
            });
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
