const SLUG_LIMIT: usize = 80;

#[must_use]
pub fn normalize_slug(text: &str) -> String {
    let lowered = text.to_lowercase();
    let words: Vec<&str> = lowered
        .split(|ch: char| !ch.is_ascii_lowercase() && !ch.is_ascii_digit())
        .filter(|word| !word.is_empty())
        .collect();
    let slug: String = words.join("-").chars().take(SLUG_LIMIT).collect();
    slug.trim_end_matches('-').to_owned()
}

pub(super) fn is_link_target(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

#[must_use]
pub fn extract_wiki_links(text: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let tail = &rest[start + 2..];
        let Some(end) = tail.find("]]") else {
            break;
        };
        let target = &tail[..end];
        if is_link_target(target) {
            let slug = normalize_slug(target);
            if !slug.is_empty() && !links.contains(&slug) {
                links.push(slug);
            }
            rest = &tail[end + 2..];
        } else {
            rest = tail;
        }
    }
    links
}
