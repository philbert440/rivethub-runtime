use chrono::NaiveDate;

use super::CurrentText;
use super::TopicTarget;
use super::WikiRouter;
use crate::topics::TopicPageError;
use crate::wiki::merge::MergeInput;
use crate::wiki::merge::Merged;
use crate::wiki::merge::MergedPage;
use crate::wiki::page::SUMMARY_LIMIT;
use crate::wiki::page::WikiHistoryEntry;
use crate::wiki::page::WikiPatch;
use crate::wiki::page::bound::ARTICLE_BUDGET;
use crate::wiki::page::bound::MergeBase;
use crate::wiki::page::bound::SECTION_BUDGET;
use crate::wiki::page::bound::fit;
use crate::wiki::page::cap_summary;

const UPDATED: &str = "Updated";
const ARTICLE_LIMIT: usize = 14_000;

impl WikiRouter<'_> {
    pub(super) async fn with_current(
        &self,
        mut target: TopicTarget,
    ) -> Result<TopicTarget, TopicPageError> {
        let Some(page) = self.topics.pages.topic_page(&target.page_slug).await? else {
            return Ok(target);
        };
        target.current = Some(match &target.section {
            None => CurrentText {
                summary: cap_summary(&page.summary, SUMMARY_LIMIT).kept,
                article: Some(fit(&page.article, ARTICLE_BUDGET).kept),
                section: None,
            },
            Some(_) => CurrentText {
                summary: String::new(),
                article: None,
                section: Some(
                    page.sections
                        .iter()
                        .find(|section| section.topic == Some(target.topic))
                        .map(|section| fit(&section.body, SECTION_BUDGET).kept)
                        .unwrap_or_default(),
                ),
            },
        });
        Ok(target)
    }

    pub(super) async fn merge_entity(
        &self,
        page: &TopicTarget,
        patches: Vec<WikiPatch>,
        bases: &mut Vec<MergedPage>,
    ) -> Result<Vec<WikiPatch>, TopicPageError> {
        let Some(merger) = self.merger else {
            return Ok(patches);
        };
        let Some(current) = self.topics.pages.topic_page(&page.page_slug).await? else {
            return Ok(patches);
        };
        if current.summary.trim().is_empty() && current.article.trim().is_empty() {
            return Ok(patches);
        }
        let base = MergeBase {
            summary: cap_summary(&current.summary, SUMMARY_LIMIT).kept,
            article: fit(&current.article, ARTICLE_BUDGET).kept,
        };
        let input = MergeInput {
            title: page.name.clone(),
            summary: base.summary.clone(),
            article: base.article.clone(),
            material: patches.iter().map(material).collect(),
            limit: ARTICLE_LIMIT,
            summary_only: false,
        };
        #[cfg_attr(
            dylint_lib = "rr_lints",
            expect(
                fallback,
                reason = "PATTERNS FALLBACK: the rule of contract note 100: an entity merge that fails falls back to the patches as the model wrote them"
            )
        )]
        let Ok(result) = merger.merge(&input).await else {
            return Ok(patches);
        };
        if result.summary.trim().is_empty() {
            return Ok(patches);
        }
        let Some(patch) = combined(&patches, result, self.now.date_naive()) else {
            return Ok(patches);
        };
        bases.push(MergedPage {
            slug: page.page_slug.clone(),
            base,
        });
        Ok(vec![patch])
    }
}

fn material(patch: &WikiPatch) -> String {
    let mut text = format!("### {}\n\n{}", patch.title.trim(), patch.summary.trim());
    if let Some(article) = patch
        .article
        .as_deref()
        .map(str::trim)
        .filter(|article| !article.is_empty())
    {
        text.push_str("\n\n");
        text.push_str(article);
    }
    if let Some(entry) = &patch.history_entry {
        text.push_str("\n\nHistory ");
        text.push_str(&entry.date.to_string());
        text.push_str(": ");
        text.push_str(entry.title.trim());
        text.push_str(". ");
        text.push_str(entry.body.trim());
    }
    text
}

fn combined(patches: &[WikiPatch], merged: Merged, today: NaiveDate) -> Option<WikiPatch> {
    let mut patch = patches.first()?.clone();
    for other in patches.iter().skip(1) {
        extend(&mut patch.aliases, &other.aliases);
        extend(&mut patch.tags, &other.tags);
        extend(&mut patch.entities, &other.entities);
        if let Some(related) = &other.related {
            extend(patch.related.get_or_insert_with(Vec::new), related);
        }
    }
    let entry = patches
        .iter()
        .rev()
        .find_map(|patch| patch.history_entry.clone());
    merged.summary.trim().clone_into(&mut patch.summary);
    patch.article = Some(merged.article.trim().to_owned()).filter(|article| !article.is_empty());
    patch.history_entry = merged
        .history
        .map(|line| line.trim().to_owned())
        .filter(|line| !line.is_empty())
        .map(|body| WikiHistoryEntry {
            date: entry.as_ref().map_or(today, |entry| entry.date),
            title: entry
                .as_ref()
                .map_or_else(|| UPDATED.to_owned(), |entry| entry.title.clone()),
            body,
        });
    Some(patch)
}

fn extend<T: PartialEq + Clone>(target: &mut Vec<T>, more: &[T]) {
    for item in more {
        if !target.contains(item) {
            target.push(item.clone());
        }
    }
}
