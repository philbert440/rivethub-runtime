use std::collections::BTreeSet;

use kernel::filing::Answer;
use kernel::filing::CorrectionExample;
use kernel::filing::Subject;
use kernel::filing::Target;
use kernel::filing::TopicLine;
use kernel::topics::Slug;
use kernel::topics::TopicId;
use kernel::topics::TopicListing;

use super::FILING_SYSTEM_PROMPT;
use super::FilingPlan;
use super::parse;
use super::prompt;
use super::settle;
use crate::cleanup::ModelCall;
use crate::error::WorkerError;

const TOKENS_PER_ANSWER: u32 = 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchItem {
    pub target: Target,
    pub subject: Subject,
    pub named: Option<TopicId>,
}

pub async fn file_batch(
    call: &ModelCall<'_>,
    listings: &[TopicListing],
    lines: &[TopicLine],
    corrections: &[CorrectionExample],
    items: &[BatchItem],
    deleted: &BTreeSet<Slug>,
) -> Result<Vec<Result<FilingPlan, WorkerError>>, WorkerError> {
    let tree = prompt::Tree::of(listings, lines);
    let subjects: Vec<(&Subject, Option<TopicId>)> = items
        .iter()
        .map(|item| (&item.subject, item.named))
        .collect();
    let budget = u32::try_from(items.len())
        .map_err(|_| WorkerError::Count)?
        .saturating_mul(TOKENS_PER_ANSWER)
        .saturating_add(TOKENS_PER_ANSWER);
    let reply = call
        .ask(
            FILING_SYSTEM_PROMPT,
            &prompt::batch_prompt(&tree, corrections, &subjects),
            budget,
        )
        .await?;
    let answers = parse::batch(&reply, &tree, items.len())?;
    Ok(items
        .iter()
        .zip(answers)
        .map(|(item, answer)| {
            let answer = answer?;
            let removed = match &answer {
                Answer::Suggest { name, .. } => {
                    Slug::of_heading(name.as_str()).is_some_and(|slug| deleted.contains(&slug))
                }
                Answer::File(_) | Answer::Nothing => false,
            };
            Ok(settle(
                answer,
                item.target.clone(),
                listings,
                item.subject.head.title.as_deref(),
                removed,
            ))
        })
        .collect())
}
