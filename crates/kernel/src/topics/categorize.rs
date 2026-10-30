use std::collections::BTreeMap;
use std::collections::BTreeSet;

use super::Redirect;
use super::Topic;
use super::TopicId;
use super::TopicKind;
use super::TopicListing;
use super::TopicTree;
use super::values::fold;

const MIN_KEY_CHARS: usize = 3;
pub const GENERIC_SHARE_PERCENT: usize = 5;

#[must_use]
pub fn mentioned(
    topics: &[TopicListing],
    redirects: &[Redirect],
    texts: &[&str],
    generic: &BTreeSet<String>,
) -> Vec<TopicId> {
    let folded: Vec<String> = texts.iter().map(|text| fold(text)).collect();
    let hits: Vec<&Topic> = topics
        .iter()
        .map(|listing| &listing.topic)
        .filter(|topic| topic.kind == TopicKind::Work)
        .filter(|topic| {
            keys(topic, redirects)
                .filter(|key| !generic.contains(key))
                .any(|key| folded.iter().any(|text| mentions(text, &key)))
        })
        .collect();
    let tree = TopicTree::of(topics.iter().map(|listing| &listing.topic));
    hits.iter()
        .filter(|topic| {
            !hits
                .iter()
                .any(|other| other.id != topic.id && tree.within(other.id, topic.id))
        })
        .map(|topic| topic.id)
        .collect()
}

#[must_use]
pub fn generic_keys(
    topics: &[TopicListing],
    redirects: &[Redirect],
    corpus: &[Vec<&str>],
) -> BTreeSet<String> {
    let keys: BTreeSet<String> = topics
        .iter()
        .filter(|listing| listing.topic.kind == TopicKind::Work)
        .flat_map(|listing| keys(&listing.topic, redirects).collect::<Vec<_>>())
        .filter(|key| key.chars().count() >= MIN_KEY_CHARS)
        .collect();
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for texts in corpus {
        let folded: Vec<String> = texts.iter().map(|text| fold(text)).collect();
        for key in &keys {
            if folded.iter().any(|text| mentions(text, key)) {
                let count: &mut usize = counts.entry(key.as_str()).or_default();
                *count = count.saturating_add(1);
            }
        }
    }
    counts
        .into_iter()
        .filter(|(_, count)| {
            count.saturating_mul(100) > corpus.len().saturating_mul(GENERIC_SHARE_PERCENT)
        })
        .map(|(key, _)| key.to_owned())
        .collect()
}

fn keys<'a>(topic: &'a Topic, redirects: &'a [Redirect]) -> impl Iterator<Item = String> + 'a {
    std::iter::once(topic.slug.as_str().to_owned())
        .chain(topic.aliases.iter().map(|alias| fold(alias.as_str())))
        .chain(
            redirects
                .iter()
                .filter(|redirect| redirect.topic == topic.id)
                .map(|redirect| redirect.slug.as_str().to_owned()),
        )
}

fn mentions(text: &str, key: &str) -> bool {
    key.chars().count() >= MIN_KEY_CHARS
        && text.match_indices(key).any(|(at, _)| {
            let rest = &text[at + key.len()..];
            (at == 0 || text[..at].ends_with('-')) && (rest.is_empty() || rest.starts_with('-'))
        })
}
