use std::cmp::Reverse;

use super::WikiPage;
use super::patch::merge_list;

#[derive(Default)]
struct Leftovers {
    superseded: Vec<String>,
    merged: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedTopic {
    pub slug: String,
    pub title: String,
    pub current_state: String,
    pub article: String,
}

pub fn fold_text(current_state: &mut String, article: &mut String, child: FoldedTopic) {
    let mut leftovers = Leftovers::default();
    take_denser(
        current_state,
        child.current_state,
        "summary",
        &mut leftovers,
    );
    take_denser(article, child.article, "article", &mut leftovers);
    if !leftovers.superseded.is_empty() {
        append_entry(
            article,
            &format!(
                "#### Superseded parent text\n\n{}",
                leftovers.superseded.join("\n\n")
            ),
        );
    }
    let mut merged = format!("#### Merged from {}\n\n**{}**", child.slug, child.title);
    for text in leftovers.merged {
        merged.push_str("\n\n");
        merged.push_str(&text);
    }
    append_entry(article, &merged);
}

pub fn fold_page(parent: &mut WikiPage, child: WikiPage) {
    let parent_slug = parent.meta.slug.as_ref().to_owned();
    let child_slug = child.meta.slug.as_ref().to_owned();
    merge_list(
        &mut parent.meta.aliases,
        std::iter::once(child_slug.clone()).chain(child.meta.aliases),
        usize::MAX,
    );
    parent.meta.aliases.retain(|alias| *alias != parent_slug);
    merge_list(&mut parent.meta.tags, child.meta.tags, usize::MAX);
    merge_list(&mut parent.meta.entities, child.meta.entities, usize::MAX);
    merge_list(&mut parent.meta.related, child.meta.related, usize::MAX);
    parent
        .meta
        .related
        .retain(|slug| *slug != parent_slug && *slug != child_slug);
    merge_list(&mut parent.see_also, child.see_also, usize::MAX);
    parent
        .see_also
        .retain(|slug| *slug != parent_slug && *slug != child_slug);
    for source in child.meta.sources {
        if !parent.meta.sources.contains(&source) {
            parent.meta.sources.push(source);
        }
    }
    for citation in child.citations {
        if !parent
            .citations
            .iter()
            .any(|existing| existing.summary_id == citation.summary_id)
        {
            parent.citations.push(citation);
        }
    }
    parent
        .citations
        .sort_by_key(|citation| Reverse(citation.date));
    for entry in child.history {
        if !parent.history.contains(&entry) {
            parent.history.push(entry);
        }
    }
    parent.history.sort_by_key(|entry| Reverse(entry.date));
    for section in child.extra_sections {
        if !parent.extra_sections.contains(&section) {
            parent.extra_sections.push(section);
        }
    }
    fold_text(
        &mut parent.current_state,
        &mut parent.article,
        FoldedTopic {
            slug: child_slug,
            title: child.meta.title,
            current_state: child.current_state,
            article: child.article,
        },
    );
}

fn take_denser(slot: &mut String, incoming: String, label: &str, leftovers: &mut Leftovers) {
    if incoming.chars().count() > slot.chars().count() {
        let displaced = std::mem::replace(slot, incoming);
        if !displaced.trim().is_empty() {
            leftovers.superseded.push(format!(
                "**Displaced parent {label}:**\n\n{}",
                displaced.trim()
            ));
        }
    } else if !incoming.trim().is_empty() && incoming.trim() != slot.trim() {
        leftovers.merged.push(incoming.trim().to_owned());
    }
}

fn append_entry(article: &mut String, entry: &str) {
    if article.contains(entry) {
        return;
    }
    let kept = article.trim_end();
    *article = if kept.is_empty() {
        entry.to_owned()
    } else {
        format!("{kept}\n\n{entry}")
    };
}
