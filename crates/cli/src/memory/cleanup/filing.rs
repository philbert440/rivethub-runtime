use std::collections::BTreeSet;
use std::collections::HashMap;

use kernel::cleanup::Desk;
use kernel::cleanup::FiledHow;
use kernel::cleanup::Filing;
use kernel::cleanup::Grouped;
use kernel::cleanup::Lift;
use kernel::cleanup::ProposedTree;
use kernel::cleanup::SnapshotCorrection;
use kernel::cleanup::Turn;
use kernel::cleanup::filed_counts;
use kernel::cleanup::group;
use kernel::filing::CorrectionExample;
use kernel::filing::Target;
use kernel::filing::TopicLine;
use kernel::topics::Slug;
use kernel::topics::TopicId;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;
use workers::BatchItem;
use workers::FilingPlan;
use workers::WorkerError;

use super::Stage;
use super::pool;
use crate::Error;

pub(super) const ANSWERS: &str = "filing-answers.jsonl";
pub(super) const FILINGS: &str = "filings.json";
const CORRECTION_EXAMPLES: usize = 8;
const RETRY_BATCH: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Filed {
    pub(super) tree: ProposedTree,
    pub(super) lifts: Vec<Lift>,
    pub(super) filings: Vec<Filing>,
}

struct Context<'a> {
    stage: &'a Stage<'a>,
    desk: &'a Desk,
    lines: &'a [TopicLine],
    corrections: &'a [CorrectionExample],
    deleted: &'a BTreeSet<Slug>,
}

pub(super) async fn stage(stage: &Stage<'_>, tree: ProposedTree) -> Result<Filed, Error> {
    if let Some(filed) = stage.book.read::<Filed>(FILINGS).await? {
        return Ok(filed);
    }
    let desk = Desk::new(
        stage.snapshot,
        &tree,
        tree.listings(stage.snapshot.taken_at)?,
    );
    let lines = tree.lines();
    let corrections = stage
        .snapshot
        .corrections
        .iter()
        .take(CORRECTION_EXAMPLES)
        .map(SnapshotCorrection::example)
        .collect::<Result<Vec<_>, _>>()?;
    let deleted: BTreeSet<Slug> = stage
        .snapshot
        .refused
        .iter()
        .filter_map(|name| Slug::of_heading(name))
        .collect();
    let context = Context {
        stage,
        desk: &desk,
        lines: &lines,
        corrections: &corrections,
        deleted: &deleted,
    };
    let grouped = group(&stage.snapshot.conversations);
    let mut filings: Vec<Filing> = grouped
        .never
        .iter()
        .map(|conversation| Filing::new(conversation.head.id, None, FiledHow::Never))
        .collect();
    let filed = file_rounds(&context, &grouped, &mut filings).await?;
    filings.extend(grouped.same.iter().map(|(target, conversation)| {
        Filing::new(
            conversation.head.id,
            filed.get(target).copied().flatten(),
            FiledHow::SameTarget,
        )
    }));
    let mut tree = tree;
    let lifts = tree.lift(&filed_counts(&filings));
    for filing in &mut filings {
        filing.lift(&lifts);
    }
    let filed = Filed {
        tree,
        lifts,
        filings,
    };
    stage.book.write(FILINGS, &filed).await?;
    Ok(filed)
}

async fn file_rounds(
    context: &Context<'_>,
    grouped: &Grouped<'_>,
    filings: &mut Vec<Filing>,
) -> Result<HashMap<Target, Option<TopicId>>, Error> {
    let desk = context.desk;
    let mut answered: HashMap<Uuid, Filing> = context
        .stage
        .book
        .lines::<Filing>(ANSWERS)
        .await?
        .into_iter()
        .map(|filing| (filing.conversation, filing))
        .collect();
    let targets: HashMap<Uuid, &Target> = grouped
        .filable
        .iter()
        .map(|(target, conversation)| (conversation.head.id, target))
        .collect();
    let mut filed: HashMap<Target, Option<TopicId>> = HashMap::new();
    for tasks in [false, true] {
        let mut asks = Vec::new();
        for (target, conversation) in grouped
            .filable
            .iter()
            .filter(|(_, conversation)| Desk::is_task(conversation) == tasks)
        {
            let parent = if tasks {
                desk.parent_topic(conversation, &filed)
            } else {
                None
            };
            let filing = match desk.turn(conversation, target, parent) {
                Turn::Done(filing) => filing,
                Turn::Ask => {
                    if let Some(filing) = answered.remove(&conversation.head.id) {
                        filing
                    } else {
                        asks.push(BatchItem {
                            target: target.clone(),
                            subject: conversation.subject(),
                        });
                        continue;
                    }
                }
            };
            filed.insert(target.clone(), filing.topic);
            filings.push(filing);
        }
        for filing in ask_all(context, asks).await? {
            if let Some(target) = targets.get(&filing.conversation) {
                filed.insert((*target).clone(), filing.topic);
            }
            filings.push(filing);
        }
    }
    Ok(filed)
}

async fn ask_all(context: &Context<'_>, asks: Vec<BatchItem>) -> Result<Vec<Filing>, Error> {
    let stage = context.stage;
    let mut filings = Vec::new();
    let mut waiting = asks;
    let mut failed = 0;
    for size in [stage.settings.batch, RETRY_BATCH] {
        if waiting.is_empty() {
            break;
        }
        stage
            .book
            .note(&format!(
                "filing: asking the model about {} conversation(s) in batches of {size}",
                waiting.len()
            ))
            .await?;
        let batches: Vec<Vec<BatchItem>> = waiting.chunks(size).map(<[_]>::to_vec).collect();
        let (fresh, tally) = pool::drive(batches, stage.settings, stage.book, ANSWERS, |batch| {
            ask(context, batch)
        })
        .await?;
        failed = tally.failed;
        let answered: BTreeSet<Uuid> = fresh.iter().map(|filing| filing.conversation).collect();
        waiting.retain(|item| !answered.contains(&item.subject.head.id));
        filings.extend(fresh);
    }
    stage.incomplete("filing", failed).await?;
    let unanswered: Vec<Filing> = waiting
        .iter()
        .map(|item| Filing::new(item.subject.head.id, None, FiledHow::Unanswered))
        .collect();
    stage.book.append(ANSWERS, &unanswered).await?;
    filings.extend(unanswered);
    Ok(filings)
}

async fn ask(context: &Context<'_>, batch: Vec<BatchItem>) -> Result<Vec<Filing>, WorkerError> {
    let plans = workers::file_batch(
        context.stage.call,
        context.desk.listings(),
        context.lines,
        context.corrections,
        &batch,
        context.deleted,
    )
    .await?;
    let mut filings = Vec::new();
    for (item, plan) in batch.iter().zip(plans) {
        match plan {
            Ok(plan) => filings.push(filing_of(item.subject.head.id, plan)),
            Err(error) => tracing::warn!("cleanup filing answer skipped: {error}"),
        }
    }
    Ok(filings)
}

fn filing_of(conversation: Uuid, plan: FilingPlan) -> Filing {
    match plan {
        FilingPlan::File { topic, rule, .. } => {
            Filing::new(conversation, Some(topic), FiledHow::Rule { rule })
        }
        FilingPlan::Suggest { name, .. } => Filing::new(
            conversation,
            None,
            FiledHow::Suggested {
                name: name.as_str().to_owned(),
            },
        ),
        FilingPlan::Refused { name, refusal } => Filing::new(
            conversation,
            None,
            FiledHow::Refused {
                name: name.as_str().to_owned(),
                refusal,
            },
        ),
        FilingPlan::Placed(by) => Filing::new(conversation, None, FiledHow::Placed { by }),
        FilingPlan::Corrected => Filing::new(conversation, None, FiledHow::Corrected),
        FilingPlan::Inherits => Filing::new(conversation, None, FiledHow::Inherits),
        FilingPlan::Never => Filing::new(conversation, None, FiledHow::Never),
        FilingPlan::Unfiled => Filing::new(conversation, None, FiledHow::Nothing),
    }
}
