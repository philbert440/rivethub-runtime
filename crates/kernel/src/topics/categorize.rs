use super::Placement;
use super::Redirect;
use super::Topic;
use super::TopicError;
use super::TopicId;
use super::TopicListing;
use super::TopicName;
use super::TopicTree;
use super::values::fold;
use crate::memory::summary_title;

const MIN_KEY_CHARS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lineage {
    Root,
    Child,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstLeaf<'a> {
    pub placement: Option<&'a Placement>,
    pub lineage: Lineage,
    pub title: Option<&'a str>,
    pub summary: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    AlreadyPlaced,
    InheritsParent,
    Place(TopicId),
    Suggest {
        name: TopicName,
        topic: Option<TopicId>,
    },
    Untitled,
}

impl FirstLeaf<'_> {
    pub fn categorize(
        &self,
        topics: &[TopicListing],
        redirects: &[Redirect],
    ) -> Result<Verdict, TopicError> {
        if self.placement.is_some() {
            return Ok(Verdict::AlreadyPlaced);
        }
        if self.lineage == Lineage::Child {
            return Ok(Verdict::InheritsParent);
        }
        let title = match self.title.map(str::trim) {
            Some(title) if !title.is_empty() => Some(title.to_owned()),
            Some(_) | None => summary_title(self.summary),
        };
        let folded_title = title.as_deref().map(fold);
        let mentioned = named(
            topics,
            redirects,
            &[folded_title.as_deref(), Some(&fold(self.summary))],
        );
        if let [only] = mentioned.as_slice() {
            return Ok(Verdict::Place(*only));
        }
        let Some(title) = title else {
            return Ok(Verdict::Untitled);
        };
        let unsure = match named(topics, redirects, &[folded_title.as_deref()]).as_slice() {
            [only] => Some(*only),
            _ => None,
        };
        Ok(Verdict::Suggest {
            name: TopicName::new(title.trim_end_matches('.'))?,
            topic: unsure,
        })
    }
}

fn named(topics: &[TopicListing], redirects: &[Redirect], texts: &[Option<&str>]) -> Vec<TopicId> {
    let hits: Vec<&Topic> = topics
        .iter()
        .map(|listing| &listing.topic)
        .filter(|topic| {
            keys(topic, redirects)
                .any(|key| texts.iter().flatten().any(|text| mentions(text, &key)))
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
