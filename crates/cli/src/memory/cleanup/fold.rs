use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;

use kernel::cleanup::Fate;
use kernel::cleanup::FoldRule;
use kernel::cleanup::Folded;
use kernel::cleanup::Junk;
use kernel::cleanup::JunkReason;
use kernel::cleanup::JunkStage;
use kernel::cleanup::cited_topics;
use kernel::cleanup::near_duplicate;
use kernel::cleanup::settle_pages;
use kernel::filing::TopicLine;
use kernel::topics::TopicId;
use kernel::topics::TopicListing;
use kernel::topics::TopicName;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;
use workers::WorkerError;
use workers::cleanup::FoldAnswer;
use workers::cleanup::FoldItem;

use super::Stage;
use super::filing::Filed;
use super::pool;
use crate::Error;

pub(super) const ANSWERS: &str = "fold-answers.jsonl";
pub(super) const FOLDS: &str = "folds.json";
const FOLD_BATCH: usize = 40;
const RETRY_BATCH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Folds {
    pub(super) pages: Vec<Folded>,
    pub(super) sections: Vec<Folded>,
    pub(super) junk: Vec<Junk>,
}

#[derive(Debug, Clone)]
struct Open<'a> {
    key: String,
    title: &'a str,
    lead: String,
    entities: &'a [String],
    cited: Vec<(TopicId, u32)>,
    citations: usize,
}

struct Context<'a> {
    stage: &'a Stage<'a>,
    listings: &'a [TopicListing],
    lines: &'a [TopicLine],
}

pub(super) async fn stage(stage: &Stage<'_>, filed: &Filed) -> Result<Folds, Error> {
    if let Some(folds) = stage.book.read::<Folds>(FOLDS).await? {
        return Ok(folds);
    }
    let snapshot = stage.snapshot;
    let by_conversation: BTreeMap<Uuid, TopicId> = filed
        .filings
        .iter()
        .filter_map(|filing| filing.topic.map(|topic| (filing.conversation, topic)))
        .collect();
    let settled = settle_pages(&snapshot.pages, &by_conversation);
    let mut open: Vec<Open<'_>> = settled
        .open
        .iter()
        .filter_map(|index| snapshot.pages.get(*index))
        .map(|page| Open {
            key: page.slug.clone(),
            title: &page.title,
            lead: page.lead.clone(),
            entities: &page.entities,
            cited: cited_topics(page, &by_conversation),
            citations: page.cited.len(),
        })
        .collect();
    let section_keys: BTreeSet<String> = snapshot
        .sections
        .iter()
        .map(|section| format!("{}#{}", section.page, section.heading))
        .collect();
    open.extend(snapshot.sections.iter().map(|section| Open {
        key: format!("{}#{}", section.page, section.heading),
        title: &section.heading,
        lead: format!(
            "(an old entity section on the {} page) {}",
            section.page, section.lead
        ),
        entities: &[],
        cited: Vec::new(),
        citations: 0,
    }));
    let listings = filed.tree.listings(snapshot.taken_at)?;
    let lines = filed.tree.lines();
    let context = Context {
        stage,
        listings: &listings,
        lines: &lines,
    };
    let mut folded = settled.folded;
    folded.extend(ask_all(&context, open).await?);
    let junk = junk(stage, filed, &folded)?;
    let (sections, pages) = folded
        .into_iter()
        .partition(|folded| section_keys.contains(&folded.key));
    let folds = Folds {
        pages,
        sections,
        junk,
    };
    stage.book.write(FOLDS, &folds).await?;
    Ok(folds)
}

async fn ask_all(context: &Context<'_>, open: Vec<Open<'_>>) -> Result<Vec<Folded>, Error> {
    let stage = context.stage;
    let mut answered: HashMap<String, Folded> = stage
        .book
        .lines::<Folded>(ANSWERS)
        .await?
        .into_iter()
        .map(|folded| (folded.key.clone(), folded))
        .collect();
    let mut folded = Vec::new();
    let mut waiting = Vec::new();
    for item in open {
        match answered.remove(&item.key) {
            Some(answer) => folded.push(answer),
            None => waiting.push(item),
        }
    }
    let mut failed = 0;
    for size in [FOLD_BATCH, RETRY_BATCH] {
        if waiting.is_empty() {
            break;
        }
        stage
            .book
            .note(&format!(
                "fold: asking the model about {} page(s) in batches of {size}",
                waiting.len()
            ))
            .await?;
        let batches: Vec<Vec<Open<'_>>> = waiting.chunks(size).map(<[_]>::to_vec).collect();
        let (fresh, tally) = pool::drive(batches, stage.settings, stage.book, ANSWERS, |batch| {
            ask(context, batch)
        })
        .await?;
        failed = tally.failed;
        let done: BTreeSet<&str> = fresh.iter().map(|folded| folded.key.as_str()).collect();
        waiting.retain(|item| !done.contains(item.key.as_str()));
        folded.extend(fresh);
    }
    stage.incomplete("fold", failed).await?;
    let undecided: Vec<Folded> = waiting
        .iter()
        .map(|item| Folded {
            key: item.key.clone(),
            title: item.title.to_owned(),
            fate: Fate::Undecided,
            rule: FoldRule::Model,
        })
        .collect();
    stage.book.append(ANSWERS, &undecided).await?;
    folded.extend(undecided);
    Ok(folded)
}

async fn ask(context: &Context<'_>, batch: Vec<Open<'_>>) -> Result<Vec<Folded>, WorkerError> {
    let items: Vec<FoldItem<'_>> = batch
        .iter()
        .map(|open| FoldItem {
            title: open.title,
            lead: &open.lead,
            entities: open.entities,
            cited: open.cited.clone(),
            citations: open.citations,
        })
        .collect();
    let answers =
        workers::cleanup::fold_batch(context.stage.call, context.listings, context.lines, &items)
            .await?;
    let mut folded = Vec::new();
    for (open, answer) in batch.iter().zip(answers) {
        let fate = match answer {
            Ok(FoldAnswer::Entity(kind)) => Fate::Entity { kind },
            Ok(FoldAnswer::Section(topic)) => Fate::Section { topic },
            Ok(FoldAnswer::Topic { name, under }) => Fate::Topic {
                name: name.as_str().to_owned(),
                under,
            },
            Ok(FoldAnswer::Merge(index)) => match batch.get(index) {
                Some(into) if into.key != open.key => Fate::Merge {
                    into: into.key.clone(),
                },
                Some(_) | None => {
                    tracing::warn!("cleanup fold answer merges {} into itself", open.key);
                    continue;
                }
            },
            Ok(FoldAnswer::Remove) => Fate::Remove,
            Err(error) => {
                tracing::warn!("cleanup fold answer skipped: {error}");
                continue;
            }
        };
        folded.push(Folded {
            key: open.key.clone(),
            title: open.title.to_owned(),
            fate,
            rule: FoldRule::Model,
        });
    }
    Ok(folded)
}

fn junk(stage: &Stage<'_>, filed: &Filed, folded: &[Folded]) -> Result<Vec<Junk>, Error> {
    let mut junk = Vec::new();
    for name in folded.iter().filter_map(|folded| match &folded.fate {
        Fate::Topic { name, .. } => Some(name),
        Fate::Entity { .. }
        | Fate::Section { .. }
        | Fate::Merge { .. }
        | Fate::Remove
        | Fate::Undecided => None,
    }) {
        let reason = match stage.guard.junk(&TopicName::new(name)?) {
            Some(reason) => Some(reason),
            None => filed
                .tree
                .topics
                .iter()
                .find(|topic| near_duplicate(&topic.name, name))
                .map(|topic| JunkReason::Duplicate {
                    of: filed.tree.path(topic.id),
                }),
        };
        if let Some(reason) = reason {
            junk.push(Junk {
                name: name.clone(),
                stage: JunkStage::Fold,
                reason,
            });
        }
    }
    Ok(junk)
}
