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
