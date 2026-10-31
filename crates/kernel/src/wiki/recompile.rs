use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::builder::WikiStoreError;
use super::merge::PageMerger;
use super::page::bound::{ARCHIVED_TITLE, Archived, HISTORY_LINE, history_line, is_revision};
use super::page::{WikiHistoryEntry, WikiPage, WikiSlug};

mod consolidate;

const HISTORY_KEPT: usize = 47;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecompileWiki {
    pub slug: WikiSlug,
    pub since: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecompileSource {
    pub page: WikiPage,
    pub history: Vec<WikiHistoryEntry>,
    pub summaries: Vec<SourceSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSummary {
    pub topic: Option<String>,
    pub at: DateTime<Utc>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recompiled {
    pub base: WikiPage,
    pub page: WikiPage,
    pub archived: Vec<Archived>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecompileReport {
    pub restored: usize,
    pub merges: usize,
    pub merge_failures: usize,
}

#[async_trait]
pub trait WikiRecompileStore: Send + Sync {
    async fn recompile_source(
        &self,
        slug: &WikiSlug,
        since: DateTime<Utc>,
    ) -> Result<Option<RecompileSource>, WikiStoreError>;

    async fn store_recompiled(
        &self,
        recompiled: Recompiled,
        now: DateTime<Utc>,
    ) -> Result<String, WikiStoreError>;
}

pub async fn recompile(
    store: &dyn WikiRecompileStore,
    merger: &dyn PageMerger,
    job: &RecompileWiki,
    now: DateTime<Utc>,
) -> Result<Option<RecompileReport>, WikiStoreError> {
    let Some(source) = store.recompile_source(&job.slug, job.since).await? else {
        return Ok(None);
    };
    let mut page = source.page.clone();
    let mut archived = Vec::new();
    let mut report = RecompileReport {
        restored: restore_history(&mut page, source.history, &mut archived),
        ..RecompileReport::default()
    };
    consolidate::consolidate(&mut page, &source.summaries, merger, &mut report).await;
    let message = format!(
        "wiki({}): recompile to a bounded page\n\nRestored {} history entries; {} merge(s).\nPipeline: wiki-v7",
        job.slug.as_ref(),
        report.restored,
        report.merges
    );
    store
        .store_recompiled(
            Recompiled {
                base: source.page,
                page,
                archived,
                message,
            },
            now,
        )
        .await?;
    Ok(Some(report))
}

#[must_use]
pub fn restore_history(
    page: &mut WikiPage,
    recovered: Vec<WikiHistoryEntry>,
    archived: &mut Vec<Archived>,
) -> usize {
    let before = std::mem::take(&mut page.history);
    let mut real: Vec<WikiHistoryEntry> = Vec::new();
    for entry in &before {
        if is_revision(&entry.title) {
            archived.push(Archived {
                label: format!("{} ({})", entry.title, entry.date),
                text: entry.body.trim().to_owned(),
                notice: true,
            });
        } else if !real.iter().any(|kept| same(kept, entry)) {
            real.push(entry.clone());
        }
    }
    for entry in recovered {
        let skip = is_revision(&entry.title)
            || entry.title == ARCHIVED_TITLE
            || real.iter().any(|kept| same(kept, &entry));
        if !skip {
            real.push(entry);
        }
    }
    real.sort_by_key(|entry| std::cmp::Reverse(entry.date));
    let spilled = real.split_off(HISTORY_KEPT.min(real.len()));
    let known = |entry: &WikiHistoryEntry| before.iter().any(|kept| same(kept, entry));
    for entry in spilled.into_iter().filter(|entry| !known(entry)) {
        archived.push(history_item(&entry));
    }
    let mut restored = 0usize;
    for entry in &mut real {
        if known(entry) {
            continue;
        }
        restored = restored.saturating_add(1);
        if entry.body.chars().count() > HISTORY_LINE {
            archived.push(history_item(entry));
            entry.body = history_line(&entry.body).kept;
        }
    }
    page.history = real;
    restored
}

fn history_item(entry: &WikiHistoryEntry) -> Archived {
    Archived {
        label: format!("History {} — {}", entry.date, entry.title),
        text: entry.body.trim().to_owned(),
        notice: false,
    }
}

fn same(left: &WikiHistoryEntry, right: &WikiHistoryEntry) -> bool {
    left.date == right.date && left.title.trim() == right.title.trim()
}
