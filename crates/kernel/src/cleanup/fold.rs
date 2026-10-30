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
    Keep,
    Undecided,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum FoldRule {
    Duplicate,
    SelfEntity,
    Citations,
    Variant,
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
    if code_name(&page.title) {
        return None;
    }
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

const MIN_TIMESTAMP_DIGITS: usize = 8;
const MANY_HUMPS: usize = 3;
const MAX_VARIANT_WORDS: usize = 2;
const MIN_MAIN_CHARS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
enum JunkPageWord {
    Smoke,
    Test,
    Tests,
    Tmp,
    Temp,
    Scratch,
    Dummy,
    Placeholder,
    Untitled,
}

#[must_use]
pub fn code_name(title: &str) -> bool {
    let trimmed = title.trim();
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    match words.as_slice() {
        [word] => {
            let humps = word
                .chars()
                .zip(word.chars().skip(1))
                .filter(|(before, after)| before.is_lowercase() && after.is_uppercase())
                .count();
            let camel = word.starts_with(char::is_lowercase) && humps > 0;
            let pascal = word.starts_with(char::is_uppercase) && humps + 1 >= MANY_HUMPS;
            camel || pascal || word.contains(['_', '(', ':'])
        }
        [_, last] => last.eq_ignore_ascii_case("tool"),
        _ => false,
    }
}

#[must_use]
pub fn clear_junk(title: &str, lead: &str, citations: usize) -> bool {
    let empty = lead.trim().is_empty() && citations == 0;
    empty
        || fold(title).split('-').any(|word| {
            word.parse::<JunkPageWord>().is_ok()
                || (word.chars().count() >= MIN_TIMESTAMP_DIGITS
                    && word.chars().all(|ch| ch.is_ascii_digit()))
        })
}

#[must_use]
pub fn guard_removal(fate: Fate, junk: bool, cited: &[(TopicId, u32)]) -> Fate {
    if fate != Fate::Remove || junk {
        return fate;
    }
    cited
        .first()
        .map_or(Fate::Keep, |(topic, _)| Fate::Section { topic: *topic })
}

pub fn fold_variants(folded: &mut [Folded]) {
    let entities: Vec<(usize, EntityType, Vec<String>)> = folded
        .iter()
        .enumerate()
        .filter_map(|(index, item)| match item.fate {
            Fate::Entity { kind } => Some((index, kind, title_words(&item.title))),
            Fate::Topic { .. }
            | Fate::Section { .. }
            | Fate::Merge { .. }
            | Fate::Remove
            | Fate::Keep
            | Fate::Undecided => None,
        })
        .filter(|(_, _, words)| !words.is_empty())
        .collect();
    let mut merges = Vec::new();
    for (index, kind, words) in &entities {
        let mains: Vec<usize> = entities
            .iter()
            .filter(|(main, main_kind, main_words)| {
                main != index
                    && main_kind == kind
                    && main_words.len() < words.len()
                    && words.len() - main_words.len() <= MAX_VARIANT_WORDS
                    && main_words.concat().chars().count() >= MIN_MAIN_CHARS
                    && (words.starts_with(main_words) || words.ends_with(main_words))
            })
            .map(|(main, _, _)| *main)
            .collect();
        if let [main] = mains.as_slice() {
            merges.push((*index, *main));
        }
    }
    for (index, main) in merges {
        let Some(into) = folded.get(main).map(|main| main.key.clone()) else {
            continue;
        };
        if let Some(item) = folded.get_mut(index) {
            item.fate = Fate::Merge { into };
            item.rule = FoldRule::Variant;
        }
    }
}

fn title_words(title: &str) -> Vec<String> {
    fold(title)
        .split('-')
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
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

    #[test]
    fn only_clear_junk_is_removed_and_variants_fold_into_their_main_entity() {
        let topic = TopicId::random();
        assert_eq!(
            guard_removal(
                Fate::Remove,
                clear_junk("Market Watcher", "Tracks filings.", 2),
                &[(topic, 2)]
            ),
            Fate::Section { topic }
        );
        assert_eq!(
            guard_removal(
                Fate::Remove,
                clear_junk("Desktop smoke 1793022748807", "", 0),
                &[]
            ),
            Fate::Remove
        );
        assert!(
            code_name("AskUserQuestion")
                && code_name("getSessionHistory")
                && code_name("Read tool")
        );
        assert!(!code_name("AcmeKit") && !code_name("Release Flow"));
        let entity = |key: &str, title: &str, kind| Folded {
            key: key.to_owned(),
            title: title.to_owned(),
            fate: Fate::Entity { kind },
            rule: FoldRule::SelfEntity,
        };
        let mut folded = vec![
            entity("loader", "Loader", EntityType::ClisAndTools),
            entity("acmebuild", "acmebuild", EntityType::Hosts),
            entity(
                "acmebuild-loader",
                "acmebuild Loader",
                EntityType::ClisAndTools,
            ),
            entity("loader-cache", "Loader Cache", EntityType::Services),
        ];
        fold_variants(&mut folded);
        let fates: Vec<&Fate> = folded.iter().map(|item| &item.fate).collect();
        assert_eq!(
            fates,
            [
                &Fate::Entity {
                    kind: EntityType::ClisAndTools
                },
                &Fate::Entity {
                    kind: EntityType::Hosts
                },
                &Fate::Merge {
                    into: "loader".to_owned()
                },
                &Fate::Entity {
                    kind: EntityType::Services
                },
            ]
        );
    }
}
