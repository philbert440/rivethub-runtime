use super::Redirect;
use super::Topic;
use super::TopicId;
use super::TopicKind;
use super::TopicListing;
use super::TopicTree;
use super::values::fold;

const MIN_KEY_CHARS: usize = 3;

#[must_use]
pub fn mentioned(topics: &[TopicListing], redirects: &[Redirect], texts: &[&str]) -> Vec<TopicId> {
    let folded: Vec<String> = texts.iter().map(|text| fold(text)).collect();
    let hits: Vec<&Topic> = topics
        .iter()
        .map(|listing| &listing.topic)
        .filter(|topic| topic.kind == TopicKind::Work)
        .filter(|topic| {
            keys(topic, redirects).any(|key| folded.iter().any(|text| mentions(text, &key)))
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
