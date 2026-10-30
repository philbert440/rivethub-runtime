pub(super) const ALIASES: usize = 32;
pub(super) const TAGS: usize = 24;
pub(super) const RELATED: usize = 48;
pub(super) const CITATIONS: usize = 40;
pub(super) const HISTORY: usize = 48;
pub(super) const SOURCES: usize = 64;

pub(super) fn first<T>(items: &[T], limit: usize) -> &[T] {
    items.get(..limit).unwrap_or(items)
}

pub(super) fn last<T>(items: &[T], limit: usize) -> &[T] {
    items
        .get(items.len().saturating_sub(limit)..)
        .unwrap_or(items)
}
