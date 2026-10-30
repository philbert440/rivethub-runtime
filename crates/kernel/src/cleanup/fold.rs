use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

use super::EntityType;
use super::KnowledgePage;
use crate::topics::TopicId;
use crate::topics::fold;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "fate", rename_all = "kebab-case")]
pub enum Fate {
    Entity {
        kind: EntityType,
    },
    Topic {
        name: String,
        under: Option<TopicId>,
    },
    Section {
        topic: TopicId,
    },
    Merge {
        into: String,
    },
    Remove,
    Undecided,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum FoldRule {
    Duplicate,
    SelfEntity,
    Citations,
    Model,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folded {
    pub key: String,
    pub title: String,
    #[serde(flatten)]
    pub fate: Fate,
    pub rule: FoldRule,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settled {
    pub folded: Vec<Folded>,
    pub open: Vec<usize>,
}

#[must_use]
pub fn settle_pages(pages: &[KnowledgePage], filed: &BTreeMap<Uuid, TopicId>) -> Settled {
    let canonical = canonical_pages(pages);
    let mut settled = Settled::default();
    for (index, page) in pages.iter().enumerate() {
        let fate = match canonical.get(&fold(&page.title)) {
            Some(into) if *into != page.slug => {
                Some((Fate::Merge { into: into.clone() }, FoldRule::Duplicate))
            }
            Some(_) | None => self_entity(page)
                .map(|kind| (Fate::Entity { kind }, FoldRule::SelfEntity))
                .or_else(|| {
                    cited_topic(page, filed)
                        .map(|topic| (Fate::Section { topic }, FoldRule::Citations))
                }),
        };
        match fate {
            Some((fate, rule)) => settled.folded.push(Folded {
                key: page.slug.clone(),
                title: page.title.clone(),
                fate,
                rule,
            }),
            None => settled.open.push(index),
        }
    }
    settled
}

#[must_use]
pub fn cited_topics(page: &KnowledgePage, filed: &BTreeMap<Uuid, TopicId>) -> Vec<(TopicId, u32)> {
    let mut counts: BTreeMap<TopicId, u32> = BTreeMap::new();
    for topic in page
        .cited
        .iter()
        .filter_map(|conversation| filed.get(conversation))
    {
        let count: &mut u32 = counts.entry(*topic).or_default();
        *count = count.saturating_add(1);
    }
    let mut ranked: Vec<(TopicId, u32)> = counts.into_iter().collect();
    ranked.sort_by_key(|(_, count)| Reverse(*count));
    ranked
}

fn cited_topic(page: &KnowledgePage, filed: &BTreeMap<Uuid, TopicId>) -> Option<TopicId> {
    match cited_topics(page, filed).as_slice() {
        [(topic, _)] => Some(*topic),
        [(topic, first), (_, second), ..] if first > second => Some(*topic),
        _ => None,
    }
}

fn self_entity(page: &KnowledgePage) -> Option<EntityType> {
    let title = fold(&page.title);
    page.entities
        .iter()
        .filter_map(|reference| EntityType::of_reference(reference))
        .find(|(_, name)| *name == page.slug || *name == title)
        .map(|(kind, _)| kind)
}

fn canonical_pages(pages: &[KnowledgePage]) -> HashMap<String, String> {
    let mut groups: HashMap<String, Vec<&KnowledgePage>> = HashMap::new();
    for page in pages {
        groups.entry(fold(&page.title)).or_default().push(page);
    }
    groups
        .into_iter()
        .filter(|(_, group)| group.len() > 1)
        .filter_map(|(title, group)| {
            group
                .into_iter()
                .max_by_key(|page| (page.cited.len(), Reverse(page.slug.len())))
                .map(|page| (title, page.slug.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(slug: &str, title: &str, entities: &[&str], cited: &[Uuid]) -> KnowledgePage {
        KnowledgePage {
            slug: slug.to_owned(),
            title: title.to_owned(),
            lead: String::new(),
            entities: entities.iter().map(|entity| (*entity).to_owned()).collect(),
            cited: cited.to_vec(),
        }
    }

    #[test]
    fn settles_duplicates_entities_and_cited_sections_before_the_model() {
        let (first, second, third) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let topic = TopicId::random();
        let other = TopicId::random();
        let filed = BTreeMap::from([(first, topic), (second, topic), (third, other)]);
        let pages = [
            page("acmebuild", "acmebuild", &["host:acmebuild"], &[first]),
            page("release-flow", "Release flow", &[], &[first, second, third]),
            page("release-flow-2", "Release Flow", &[], &[third]),
            page("tie", "Tie", &[], &[first, third]),
            page("loose", "Loose", &["project:acmekit"], &[]),
        ];
        let settled = settle_pages(&pages, &filed);
        let fates: Vec<(&str, &Fate)> = settled
            .folded
            .iter()
            .map(|folded| (folded.key.as_str(), &folded.fate))
            .collect();
        assert_eq!(
            fates,
            [
                (
                    "acmebuild",
                    &Fate::Entity {
                        kind: EntityType::Hosts
                    }
                ),
                ("release-flow", &Fate::Section { topic }),
                (
                    "release-flow-2",
                    &Fate::Merge {
                        into: "release-flow".to_owned()
                    }
                ),
            ]
        );
        assert_eq!(settled.open, [3, 4]);
    }
}
