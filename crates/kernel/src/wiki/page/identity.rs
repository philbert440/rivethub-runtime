use super::slug::normalize_slug;
#[must_use]
pub fn is_slug_variant(left: &str, right: &str) -> bool {
    let left = normalize_slug(left);
    let right = normalize_slug(right);
    if left.is_empty() || right.is_empty() || left == right {
        return left == right && !left.is_empty();
    }
    is_child(&left, &right) || is_child(&right, &left)
}

fn is_child(child: &str, parent: &str) -> bool {
    child
        .strip_prefix(parent)
        .is_some_and(|rest| rest.starts_with('-'))
}
#[must_use]
pub fn find_stem_match<S: AsRef<str>>(proposed: &str, existing: &[S]) -> Option<String> {
    let proposed = normalize_slug(proposed);
    if proposed.is_empty() {
        return None;
    }
    existing
        .iter()
        .map(|slug| normalize_slug(slug.as_ref()))
        .filter(|slug| !slug.is_empty() && (*slug == proposed || is_slug_variant(slug, &proposed)))
        .min_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
enum NoiseWord {
    Rivet,
    Rivetos,
    Rivethub,
    Project,
    Projects,
    The,
    And,
    Of,
    For,
    In,
    On,
    To,
    With,
    Via,
    By,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicIdentity {
    pub key: String,
    pub noise: usize,
}

impl TopicIdentity {
    #[must_use]
    pub fn probe(&self) -> Option<&str> {
        self.key
            .split('-')
            .max_by_key(|word| word.len())
            .map(|word| word.strip_suffix('y').unwrap_or(word))
            .filter(|word| !word.is_empty())
    }

    #[must_use]
    pub fn closest<I: IntoIterator<Item = (String, String)>>(
        &self,
        candidates: I,
    ) -> Option<String> {
        candidates
            .into_iter()
            .filter_map(|(slug, title)| {
                let found = topic_identity(&slug, &title);
                (found.key == self.key).then_some((found.noise, slug))
            })
            .min_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| left.1.len().cmp(&right.1.len()))
                    .then_with(|| left.1.cmp(&right.1))
            })
            .map(|(_, slug)| slug)
    }
}

#[must_use]
pub fn topic_identity(slug: &str, title: &str) -> TopicIdentity {
    let slug = normalize_slug(slug);
    let title = normalize_slug(title);
    let mut words: Vec<&str> = slug.split('-').filter(|word| !word.is_empty()).collect();
    let total = words.len();
    if let [_, .., last] = words.as_slice()
        && is_dedupe_suffix(last)
        && !title.split('-').any(|word| word == *last)
    {
        words.pop();
    }
    let mut kept: Vec<String> = words
        .into_iter()
        .filter(|word| word.parse::<NoiseWord>().is_err())
        .map(singular)
        .collect();
    if kept.is_empty() {
        return TopicIdentity {
            key: slug,
            noise: 0,
        };
    }
    let noise = total - kept.len();
    kept.sort_unstable();
    kept.dedup();
    TopicIdentity {
        key: kept.join("-"),
        noise,
    }
}

fn is_dedupe_suffix(word: &str) -> bool {
    matches!(word.as_bytes(), [b'2'..=b'9'])
}

fn singular(word: &str) -> String {
    if word.bytes().any(|byte| byte.is_ascii_digit()) {
        return word.to_owned();
    }
    if let Some(stem) = word.strip_suffix("ies")
        && stem.len() > 1
    {
        return format!("{stem}y");
    }
    if let Some(stem) = word.strip_suffix("sses") {
        return format!("{stem}ss");
    }
    if let Some(stem) = word.strip_suffix("xes") {
        return format!("{stem}x");
    }
    match word.strip_suffix('s') {
        Some(stem) if stem.len() > 2 && !stem.ends_with(['s', 'u', 'i']) => stem.to_owned(),
        _ => word.to_owned(),
    }
}
