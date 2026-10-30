use super::WikiPage;
use super::limits;

#[must_use]
pub fn build_wiki_search_text(page: &WikiPage) -> String {
    [
        page.meta.title.as_str(),
        &limits::first(&page.meta.aliases, limits::ALIASES).join(" "),
        &page.current_state,
        page.article.trim(),
        &limits::first(&page.meta.related, limits::RELATED).join(" "),
    ]
    .join(" ")
}
