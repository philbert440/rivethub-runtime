use std::cmp::Reverse;

use crate::topics::PlacementSource;
use crate::topics::Redirect;
use crate::topics::TopicId;
use crate::topics::TopicListing;
use crate::topics::mentioned;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, strum::Display, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Rule {
    Parent,
    Folder,
    Name,
    Model,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Placed(PlacementSource),
    Corrected,
    Inherits,
    File { topic: TopicId, rule: Rule },
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evidence<'a> {
    pub placed: Option<PlacementSource>,
    pub corrected: bool,
    pub child: bool,
    pub parent_topic: Option<TopicId>,
    pub cwd: Option<&'a str>,
    pub title: Option<&'a str>,
    pub summary: Option<&'a str>,
}

impl Evidence<'_> {
    #[must_use]
    pub fn decide(&self, topics: &[TopicListing], redirects: &[Redirect]) -> Step {
        if let Some(by) = self.placed {
            return Step::Placed(by);
        }
        if self.corrected {
            return Step::Corrected;
        }
        if let Some(topic) = self.parent_topic {
            return Step::File {
                topic,
                rule: Rule::Parent,
            };
        }
        if self.child {
            return Step::Inherits;
        }
        if let Some(topic) = self.cwd.and_then(|cwd| folder_topic(cwd, topics)) {
            return Step::File {
                topic,
                rule: Rule::Folder,
            };
        }
        let texts: Vec<&str> = self.title.into_iter().chain(self.summary).collect();
        match mentioned(topics, redirects, &texts).as_slice() {
            [topic] => Step::File {
                topic: *topic,
                rule: Rule::Name,
            },
            _ => Step::Ask,
        }
    }
}

fn folder_topic(cwd: &str, topics: &[TopicListing]) -> Option<TopicId> {
    let mut owners: Vec<(usize, TopicId)> = topics
        .iter()
        .filter_map(|listing| {
            let directory = listing
                .topic
                .directory
                .as_ref()?
                .path()
                .trim_end_matches('/');
            inside(cwd, directory).then_some((directory.len(), listing.topic.id))
        })
        .collect();
    owners.sort_by_key(|(depth, _)| Reverse(*depth));
    match owners.as_slice() {
        [(depth, topic), rest @ ..] if rest.first().is_none_or(|(next, _)| next < depth) => {
            Some(*topic)
        }
        _ => None,
    }
}

fn inside(cwd: &str, directory: &str) -> bool {
    !directory.is_empty()
        && cwd
            .strip_prefix(directory)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}
