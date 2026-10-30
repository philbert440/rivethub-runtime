use std::collections::BTreeMap;

use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use serde::Serialize;

use super::Junk;
use super::JunkReason;
use super::JunkStage;
use super::NameGuard;
use super::Snapshot;
use super::SnapshotRoot;
use super::names::near_duplicate;
use crate::filing::TopicLine;
use crate::topics::Alias;
use crate::topics::MAX_DEPTH;
use crate::topics::Redirect;
use crate::topics::Slug;
use crate::topics::Topic;
use crate::topics::TopicError;
use crate::topics::TopicId;
use crate::topics::TopicKind;
use crate::topics::TopicListing;
use crate::topics::TopicName;
use crate::topics::fold;

pub const MIN_SESSIONS: u32 = 3;
const PATH_SEPARATOR: &str = " › ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Origin {
    Project,
    Existing,
    Proposed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeTopic {
    pub id: TopicId,
    pub name: String,
    pub parent: Option<TopicId>,
    pub aliases: Vec<String>,
    pub about: Option<String>,
    pub origin: Origin,
    pub estimate: u32,
    pub directory: Option<SnapshotRoot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftTopic {
    pub name: String,
    pub parent: Option<String>,
    pub about: Option<String>,
    pub estimate: u32,
    pub merges: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeMerge {
    pub from: TopicId,
    pub name: String,
    pub into: TopicId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lift {
    pub from: TopicId,
    pub name: String,
    pub to: Option<TopicId>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedTree {
    pub topics: Vec<TreeTopic>,
    pub merges: Vec<TreeMerge>,
    pub junk: Vec<Junk>,
}

enum Admitted {
    Done,
    Waiting(DraftTopic),
}

impl ProposedTree {
    pub fn propose(
        snapshot: &Snapshot,
        drafts: Vec<DraftTopic>,
        guard: &NameGuard,
    ) -> Result<Self, TopicError> {
        let mut tree = Self::seeded(snapshot, guard)?;
        let mut waiting = drafts;
        loop {
            let before = waiting.len();
            let mut next = Vec::new();
            for draft in waiting {
                if let Admitted::Waiting(draft) = tree.admit(draft, guard, false)? {
                    next.push(draft);
                }
            }
            waiting = next;
            if waiting.is_empty() || waiting.len() == before {
                break;
            }
        }
        for draft in waiting {
            tree.admit(draft, guard, true)?;
        }
        let estimates: BTreeMap<TopicId, u32> = tree
            .topics
            .iter()
            .map(|topic| (topic.id, topic.estimate))
            .collect();
        tree.prune(&estimates);
        Ok(tree)
    }

    pub fn lift(&mut self, filed: &BTreeMap<TopicId, u32>) -> Vec<Lift> {
        self.prune(filed)
    }

    #[must_use]
    pub fn topic(&self, id: TopicId) -> Option<&TreeTopic> {
        self.topics.iter().find(|topic| topic.id == id)
    }

    #[must_use]
    pub fn resolve(&self, id: TopicId) -> Option<TopicId> {
        let mut current = id;
        for _ in 0..=self.merges.len() {
            match self.merges.iter().find(|merge| merge.from == current) {
                Some(merge) => current = merge.into,
                None => break,
            }
        }
        self.topic(current).map(|topic| topic.id)
    }

    #[must_use]
    pub fn path(&self, id: TopicId) -> String {
        let mut names: Vec<&str> = self
            .chain(id)
            .into_iter()
            .map(|topic| topic.name.as_str())
            .collect();
        names.reverse();
        names.join(PATH_SEPARATOR)
    }

    #[must_use]
    pub fn within(&self, id: TopicId, ancestor: TopicId) -> bool {
        self.chain(id).iter().any(|topic| topic.id == ancestor)
    }

    #[must_use]
    pub fn root(&self, id: TopicId) -> Option<TopicId> {
        self.chain(id).last().map(|topic| topic.id)
    }

    pub fn listings(&self, now: DateTime<Utc>) -> Result<Vec<TopicListing>, TopicError> {
        self.topics
            .iter()
            .map(|topic| {
                Ok(TopicListing {
                    topic: Topic {
                        id: topic.id,
                        kind: TopicKind::Work,
                        slug: Slug::fold(&topic.name)?,
                        name: TopicName::new(&topic.name)?,
                        aliases: topic
                            .aliases
                            .iter()
                            .map(|alias| Alias::new(alias))
                            .collect::<Result<_, _>>()?,
                        parent: topic.parent,
                        directory: topic.directory.as_ref().and_then(SnapshotRoot::root),
                        created_at: now,
                        updated_at: now,
                        closed_at: None,
                    },
                    session_count: u64::from(topic.estimate),
                    last_active: None,
                })
            })
            .collect()
    }

    #[must_use]
    pub fn redirects(&self, snapshot: &Snapshot) -> Vec<Redirect> {
        let merged = self.merges.iter().filter_map(|merge| {
            Some(Redirect {
                slug: Slug::of_heading(&merge.name)?,
                topic: self.resolve(merge.into)?,
            })
        });
        let kept = snapshot.redirects.iter().filter_map(|redirect| {
            Some(Redirect {
                slug: Slug::of_heading(&redirect.slug)?,
                topic: self.resolve(redirect.topic)?,
            })
        });
        merged.chain(kept).collect()
    }

    #[must_use]
    pub fn lines(&self) -> Vec<TopicLine> {
        self.topics
            .iter()
            .filter_map(|topic| {
                topic.about.as_ref().map(|about| TopicLine {
                    topic: topic.id,
                    line: about.clone(),
                })
            })
            .collect()
    }

    fn seeded(snapshot: &Snapshot, guard: &NameGuard) -> Result<Self, TopicError> {
        let mut tree = Self::default();
        for existing in snapshot
            .topics
            .iter()
            .filter(|topic| topic.kind == TopicKind::Work)
        {
            tree.topics.push(TreeTopic {
                id: existing.id,
                name: existing.name.clone(),
                parent: existing.parent,
                aliases: existing.aliases.clone(),
                about: None,
                origin: Origin::Existing,
                estimate: 0,
                directory: existing.directory.clone(),
            });
        }
        for project in &snapshot.projects {
            let name = TopicName::new(&project.name)?;
            if let Some(reason @ (JunkReason::Generic | JunkReason::Rejected)) = guard.junk(&name) {
                tree.junk(&project.name, reason);
                continue;
            }
            match tree.named(&project.name) {
                Some(id) => {
                    if let Some(topic) = tree.topics.iter_mut().find(|topic| topic.id == id) {
                        topic.aliases.extend(project.aliases.iter().cloned());
                    }
                }
                None => tree.topics.push(TreeTopic {
                    id: TopicId::random(),
                    name: project.name.clone(),
                    parent: None,
                    aliases: project.aliases.clone(),
                    about: None,
                    origin: Origin::Project,
                    estimate: 0,
                    directory: None,
                }),
            }
        }
        Ok(tree)
    }

    fn admit(
        &mut self,
        draft: DraftTopic,
        guard: &NameGuard,
        last: bool,
    ) -> Result<Admitted, TopicError> {
        let name = TopicName::new(&draft.name)?;
        if let Some(id) = self.named(name.as_str()) {
            self.grow(id, &draft);
            self.merge_named(&draft.merges, id);
            return Ok(Admitted::Done);
        }
        if let Some(reason) = guard.junk(&name) {
            self.junk(name.as_str(), reason);
            return Ok(Admitted::Done);
        }
        let twin = self
            .topics
            .iter()
            .find(|topic| near_duplicate(&topic.name, name.as_str()))
            .map(|topic| topic.id);
        if let Some(twin) = twin {
            self.grow(twin, &draft);
            let of = self.path(twin);
            self.junk(name.as_str(), JunkReason::Duplicate { of });
            return Ok(Admitted::Done);
        }
        let parent = match draft.parent.as_deref().map(|parent| self.named(parent)) {
            None => None,
            Some(Some(parent)) => Some(parent),
            Some(None) if last => None,
            Some(None) => return Ok(Admitted::Waiting(draft)),
        };
        if parent.is_some_and(|parent| self.chain(parent).len() >= MAX_DEPTH) {
            self.junk(name.as_str(), JunkReason::TooDeep);
            return Ok(Admitted::Done);
        }
        let id = TopicId::random();
        self.topics.push(TreeTopic {
            id,
            name: name.as_str().to_owned(),
            parent,
            aliases: Vec::new(),
            about: draft.about.clone(),
            origin: Origin::Proposed,
            estimate: draft.estimate,
            directory: None,
        });
        self.merge_named(&draft.merges, id);
        Ok(Admitted::Done)
    }

    fn grow(&mut self, id: TopicId, draft: &DraftTopic) {
        if let Some(topic) = self.topics.iter_mut().find(|topic| topic.id == id) {
            topic.estimate = topic.estimate.saturating_add(draft.estimate);
            if topic.about.is_none() {
                topic.about.clone_from(&draft.about);
            }
        }
    }

    fn merge_named(&mut self, names: &[String], into: TopicId) {
        for name in names {
            let Some(from) = self
                .named(name)
                .and_then(|id| self.topic(id))
                .filter(|topic| topic.origin == Origin::Existing && topic.id != into)
                .cloned()
            else {
                continue;
            };
            if self.chain(into).iter().any(|above| above.id == from.id) {
                continue;
            }
            self.topics.retain(|topic| topic.id != from.id);
            for topic in &mut self.topics {
                if topic.parent == Some(from.id) {
                    topic.parent = Some(into);
                }
            }
            if let Some(target) = self.topics.iter_mut().find(|topic| topic.id == into) {
                target.estimate = target.estimate.saturating_add(from.estimate);
                target.aliases.extend(from.aliases.iter().cloned());
            }
            self.merges.push(TreeMerge {
                from: from.id,
                name: from.name,
                into,
            });
        }
    }

    fn prune(&mut self, counts: &BTreeMap<TopicId, u32>) -> Vec<Lift> {
        let mut lifts = Vec::new();
        loop {
            let totals = self.totals(counts);
            let weak = self
                .topics
                .iter()
                .filter(|topic| topic.origin == Origin::Proposed)
                .filter(|topic| totals.get(&topic.id).copied().unwrap_or_default() < MIN_SESSIONS)
                .max_by_key(|topic| self.chain(topic.id).len())
                .cloned();
            let Some(weak) = weak else {
                break;
            };
            self.topics.retain(|topic| topic.id != weak.id);
            for topic in &mut self.topics {
                if topic.parent == Some(weak.id) {
                    topic.parent = weak.parent;
                }
            }
            let sessions = totals.get(&weak.id).copied().unwrap_or_default();
            self.junk(&weak.name, JunkReason::TooFew { sessions });
            lifts.push(Lift {
                from: weak.id,
                name: weak.name,
                to: weak.parent,
            });
        }
        lifts
    }

    fn totals(&self, counts: &BTreeMap<TopicId, u32>) -> BTreeMap<TopicId, u32> {
        let mut totals = BTreeMap::new();
        for topic in &self.topics {
            let own = counts.get(&topic.id).copied().unwrap_or_default();
            for above in self.chain(topic.id) {
                let total: &mut u32 = totals.entry(above.id).or_default();
                *total = total.saturating_add(own);
            }
        }
        totals
    }

    fn named(&self, name: &str) -> Option<TopicId> {
        let wanted = fold(name);
        if wanted.is_empty() {
            return None;
        }
        self.topics
            .iter()
            .find(|topic| {
                fold(&topic.name) == wanted
                    || topic.aliases.iter().any(|alias| fold(alias) == wanted)
            })
            .map(|topic| topic.id)
    }

    fn chain(&self, id: TopicId) -> Vec<&TreeTopic> {
        let mut chain = Vec::new();
        let mut current = self.topic(id);
        while let Some(topic) = current {
            if chain.len() > self.topics.len() {
                break;
            }
            chain.push(topic);
            current = topic.parent.and_then(|parent| self.topic(parent));
        }
        chain
    }

    fn junk(&mut self, name: &str, reason: JunkReason) {
        self.junk.push(Junk {
            name: name.to_owned(),
            stage: JunkStage::Tree,
            reason,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::Project;
    use crate::cleanup::SnapshotTopic;

    fn draft(name: &str, parent: Option<&str>, estimate: u32, merges: &[&str]) -> DraftTopic {
        DraftTopic {
            name: name.to_owned(),
            parent: parent.map(str::to_owned),
            about: None,
            estimate,
            merges: merges.iter().map(|merge| (*merge).to_owned()).collect(),
        }
    }

    #[test]
    fn seeds_projects_and_guards_the_proposed_names() {
        let now = Utc::now();
        let existing = SnapshotTopic {
            id: TopicId::random(),
            kind: TopicKind::Work,
            name: "AcmeKit Rewrite".to_owned(),
            parent: None,
            aliases: Vec::new(),
            directory: None,
            created_at: now,
            updated_at: now,
        };
        let snapshot = Snapshot {
            taken_at: now,
            conversations: Vec::new(),
            topics: vec![existing.clone()],
            redirects: Vec::new(),
            placements: Vec::new(),
            corrections: Vec::new(),
            projects: vec![
                Project {
                    name: "AcmeKit".to_owned(),
                    aliases: vec!["acmekit-main".to_owned()],
                },
                Project {
                    name: "General".to_owned(),
                    aliases: Vec::new(),
                },
            ],
            refused: Vec::new(),
            pages: Vec::new(),
            sections: Vec::new(),
        };
        let guard = NameGuard::new(["Fix the nightly build"], []);
        let drafts = vec![
            draft("Release Pipeline", Some("AcmeKit"), 9, &[]),
            draft("Release pipelines", Some("AcmeKit"), 2, &[]),
            draft("Fix the nightly build", Some("AcmeKit"), 5, &[]),
            draft("Nightly Signing", Some("Release Pipeline"), 4, &[]),
            draft("Docs Site", None, 1, &[]),
            draft("acmekit-main", None, 6, &["AcmeKit Rewrite"]),
        ];
        let tree = ProposedTree::propose(&snapshot, drafts, &guard).unwrap();
        let paths: Vec<String> = tree
            .topics
            .iter()
            .map(|topic| tree.path(topic.id))
            .collect();
        assert_eq!(
            paths,
            [
                "AcmeKit",
                "AcmeKit › Release Pipeline",
                "AcmeKit › Release Pipeline › Nightly Signing"
            ]
        );
        assert_eq!(tree.merges.len(), 1);
        assert_eq!(
            tree.resolve(existing.id),
            tree.topics.first().map(|topic| topic.id)
        );
        let reasons: Vec<(&str, &JunkReason)> = tree
            .junk
            .iter()
            .map(|junk| (junk.name.as_str(), &junk.reason))
            .collect();
        assert_eq!(
            reasons,
            [
                ("General", &JunkReason::Generic),
                (
                    "Release pipelines",
                    &JunkReason::Duplicate {
                        of: "AcmeKit › Release Pipeline".to_owned()
                    }
                ),
                ("Fix the nightly build", &JunkReason::Title),
                ("Docs Site", &JunkReason::TooFew { sessions: 1 }),
            ]
        );
    }
}
