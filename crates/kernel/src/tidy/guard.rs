use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

use super::Fingerprint;
use super::Suggested;
use super::TreeEdit;
use crate::filing::Refusal;
use crate::topics::MAX_DEPTH;
use crate::topics::MergePlan;
use crate::topics::Slug;
use crate::topics::TopicId;
use crate::topics::TopicKind;
use crate::topics::TopicListing;
use crate::topics::TopicName;
use crate::topics::TopicTree;
use crate::topics::check_parent;
use crate::wiki::page::topic_identity;

pub const MIN_SESSIONS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cluster {
    pub name: TopicName,
    pub parent: Option<TopicId>,
    pub sessions: Vec<Suggested>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    Known,
    Unknown,
    TooFewSessions { found: usize },
    Name(Refusal),
    Unnamed,
    Tombstoned,
    Taken,
    Misplaced,
    NotEmpty,
    Closed,
    Unchanged,
}

#[must_use]
pub fn clusters(suggestions: Vec<Suggested>) -> Vec<Cluster> {
    let mut grouped: BTreeMap<Slug, Vec<Suggested>> = BTreeMap::new();
    for suggested in suggestions {
        if let Some(slug) = Slug::of_heading(suggested.name.as_str()) {
            grouped.entry(slug).or_default().push(suggested);
        }
    }
    let mut clusters: Vec<Cluster> = grouped
        .into_values()
        .filter_map(|sessions| {
            let first = sessions.first()?;
            Some(Cluster {
                name: first.name.clone(),
                parent: most_common(sessions.iter().map(|suggested| suggested.parent)),
                sessions,
            })
        })
        .collect();
    clusters.sort_by_key(|cluster| Reverse(cluster.sessions.len()));
    clusters
}

#[must_use]
pub fn duplicates(listings: &[TopicListing]) -> Vec<(TopicId, TopicId)> {
    let mut grouped: BTreeMap<(TopicKind, String), Vec<&TopicListing>> = BTreeMap::new();
    for listing in listings {
        let topic = &listing.topic;
        let identity = topic_identity(topic.slug.as_str(), topic.name.as_str());
        grouped
            .entry((topic.kind, identity.key))
            .or_default()
            .push(listing);
    }
    grouped
        .into_values()
        .filter(|group| group.len() > 1)
        .flat_map(|mut group| {
            group.sort_by_key(|listing| (Reverse(listing.session_count), listing.topic.created_at));
            let into = group.first().map(|listing| listing.topic.id);
            group
                .into_iter()
                .skip(1)
                .filter_map(move |listing| Some((listing.topic.id, into?)))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn most_common(parents: impl Iterator<Item = Option<TopicId>>) -> Option<TopicId> {
    let mut counts: BTreeMap<Option<TopicId>, usize> = BTreeMap::new();
    for parent in parents {
        *counts.entry(parent).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .and_then(|(parent, _)| parent)
}

#[derive(Debug)]
pub struct Guard<'a> {
    topics: BTreeMap<TopicId, &'a TopicListing>,
    tree: TopicTree,
    known: &'a BTreeSet<Fingerprint>,
    tombstoned: &'a BTreeSet<(TopicKind, Slug)>,
}

impl<'a> Guard<'a> {
    #[must_use]
    pub fn new(
        listings: &'a [TopicListing],
        known: &'a BTreeSet<Fingerprint>,
        tombstoned: &'a BTreeSet<(TopicKind, Slug)>,
    ) -> Self {
        Self {
            topics: listings
                .iter()
                .map(|listing| (listing.topic.id, listing))
                .collect(),
            tree: TopicTree::of(listings.iter().map(|listing| &listing.topic)),
            known,
            tombstoned,
        }
    }

    #[must_use]
    pub fn named(&self, change: &TreeEdit) -> Option<(TopicKind, Slug)> {
        let (kind, name) = match change {
            TreeEdit::CreateTopic { name, .. } => (TopicKind::Work, name),
            TreeEdit::AddType { name, .. } => (TopicKind::Type, name),
            TreeEdit::Rename { topic, name } => (self.topics.get(topic)?.topic.kind, name),
            TreeEdit::Merge { .. }
            | TreeEdit::Move { .. }
            | TreeEdit::Close { .. }
            | TreeEdit::Delete { .. }
            | TreeEdit::FileInto { .. }
            | TreeEdit::MergePage { .. }
            | TreeEdit::RemovePage { .. } => return None,
        };
        Some((kind, Slug::of_heading(name.as_str())?))
    }

    pub fn check(&self, change: &TreeEdit) -> Result<(), Refused> {
        let fingerprint = change.fingerprint().map_err(|_| Refused::Unnamed)?;
        if self.known.contains(&fingerprint) {
            return Err(Refused::Known);
        }
        match change {
            TreeEdit::Merge { source, into } => {
                MergePlan::new(
                    &self.topic(*source)?.topic,
                    &self.topic(*into)?.topic,
                    &self.tree,
                )
                .map_err(|_| Refused::Misplaced)?;
                Ok(())
            }
            TreeEdit::Move { topic, parent } => self.moves(*topic, *parent),
            TreeEdit::Rename { topic, name } => {
                let current = &self.topic(*topic)?.topic;
                if Slug::of_heading(name.as_str()).as_ref() == Some(&current.slug) {
                    return Err(Refused::Unchanged);
                }
                self.fresh(current.kind, name)
            }
            TreeEdit::Close { topic } => {
                let listing = self.topic(*topic)?;
                match (listing.topic.kind, listing.topic.closed_at) {
                    (TopicKind::Work, None) => Ok(()),
                    (TopicKind::Work, Some(_)) => Err(Refused::Closed),
                    (TopicKind::Type | TopicKind::Entity, _) => Err(Refused::Misplaced),
                }
            }
            TreeEdit::Delete { topic } => {
                let listing = self.topic(*topic)?;
                if listing.session_count > 0 || !self.tree.children(*topic).is_empty() {
                    return Err(Refused::NotEmpty);
                }
                Ok(())
            }
            TreeEdit::CreateTopic {
                name,
                parent,
                members,
                ..
            } => {
                if members.len() < MIN_SESSIONS {
                    return Err(Refused::TooFewSessions {
                        found: members.len(),
                    });
                }
                self.fits_under(TopicKind::Work, *parent)?;
                self.fresh(TopicKind::Work, name)
            }
            TreeEdit::FileInto { topic, .. } => {
                self.topic(*topic)?;
                Ok(())
            }
            TreeEdit::MergePage { .. } | TreeEdit::RemovePage { .. } => Ok(()),
            TreeEdit::AddType { name, parent, .. } => {
                self.fits_under(TopicKind::Type, *parent)?;
                self.fresh(TopicKind::Type, name)
            }
        }
    }

    fn topic(&self, id: TopicId) -> Result<&'a TopicListing, Refused> {
        self.topics.get(&id).copied().ok_or(Refused::Unknown)
    }

    fn moves(&self, id: TopicId, parent: Option<TopicId>) -> Result<(), Refused> {
        let topic = &self.topic(id)?.topic;
        if topic.parent == parent {
            return Err(Refused::Unchanged);
        }
        match parent {
            Some(parent) => check_parent(topic, &self.topic(parent)?.topic, &self.tree)
                .map_err(|_| Refused::Misplaced),
            None if topic.kind == TopicKind::Entity => Err(Refused::Misplaced),
            None => Ok(()),
        }
    }

    fn fits_under(&self, kind: TopicKind, parent: Option<TopicId>) -> Result<(), Refused> {
        match parent {
            None => Ok(()),
            Some(parent) => {
                let listing = self.topic(parent)?;
                if listing.topic.kind == kind && self.tree.depth(parent) < MAX_DEPTH {
                    Ok(())
                } else {
                    Err(Refused::Misplaced)
                }
            }
        }
    }

    fn fresh(&self, kind: TopicKind, name: &TopicName) -> Result<(), Refused> {
        if let Some(refusal) = Refusal::of(name, None) {
            return Err(Refused::Name(refusal));
        }
        let slug = Slug::of_heading(name.as_str()).ok_or(Refused::Unnamed)?;
        if self.tombstoned.contains(&(kind, slug.clone())) {
            return Err(Refused::Tombstoned);
        }
        let taken = self.topics.values().any(|listing| {
            listing.topic.kind == kind
                && (listing.topic.slug == slug
                    || listing
                        .topic
                        .aliases
                        .iter()
                        .any(|alias| Slug::of_heading(alias.as_str()).as_ref() == Some(&slug)))
        });
        if taken {
            return Err(Refused::Taken);
        }
        Ok(())
    }
}
