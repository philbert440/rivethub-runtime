use std::collections::BTreeSet;

use kernel::cleanup::DraftTopic;
use kernel::cleanup::Origin;
use kernel::cleanup::ProposedTree;
use kernel::cleanup::group;
use serde::Deserialize;
use serde::Serialize;
use workers::cleanup::Candidate;
use workers::cleanup::TreeItem;
use workers::cleanup::TreeSeed;

use super::Stage;
use super::pool;
use crate::Error;

pub(super) const TREE: &str = "tree.json";
const CHUNKS: &str = "tree-chunks.jsonl";
const DRAFTS: &str = "tree-drafts.jsonl";
const CHUNK: usize = 250;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChunkDone {
    chunk: usize,
    candidates: Vec<Candidate>,
}

pub(super) async fn stage(stage: &Stage<'_>) -> Result<ProposedTree, Error> {
    if let Some(tree) = stage.book.read::<ProposedTree>(TREE).await? {
        return Ok(tree);
    }
    let seeds = seeds(stage)?;
    let grouped = group(&stage.snapshot.conversations);
    let items: Vec<TreeItem<'_>> = grouped
        .filable
        .iter()
        .map(|(_, conversation)| TreeItem {
            title: conversation.title(),
            cwd: conversation.cwd.as_deref(),
            summary: conversation.summary.as_deref(),
        })
        .collect();
    let mut done: Vec<ChunkDone> = stage.book.lines(CHUNKS).await?;
    let finished: BTreeSet<usize> = done.iter().map(|chunk| chunk.chunk).collect();
    let jobs: Vec<(usize, &[TreeItem<'_>])> = items
        .chunks(CHUNK)
        .enumerate()
        .filter(|(index, _)| !finished.contains(index))
        .collect();
    stage
        .book
        .note(&format!("tree: {} chunk(s) to propose", jobs.len()))
        .await?;
    let seeds_ref = &seeds;
    let (fresh, tally) = pool::drive(
        jobs,
        stage.settings,
        stage.book,
        CHUNKS,
        |(chunk, items)| async move {
            let candidates = workers::cleanup::propose_chunk(stage.call, seeds_ref, items).await?;
            Ok(vec![ChunkDone { chunk, candidates }])
        },
    )
    .await?;
    stage.incomplete("tree", tally.failed).await?;
    done.extend(fresh);
    let candidates: Vec<Candidate> = done
        .into_iter()
        .flat_map(|chunk| chunk.candidates)
        .collect();
    let drafts = drafts(stage, &seeds, candidates).await?;
    let tree = ProposedTree::propose(stage.snapshot, drafts, stage.guard)?;
    stage
        .book
        .forget(&[
            super::filing::ANSWERS,
            super::filing::FILINGS,
            super::fold::ANSWERS,
            super::fold::FOLDS,
        ])
        .await?;
    stage.book.write(TREE, &tree).await?;
    stage
        .book
        .note(&format!("tree: {} topics proposed", tree.topics.len()))
        .await?;
    Ok(tree)
}

async fn drafts(
    stage: &Stage<'_>,
    seeds: &[TreeSeed],
    candidates: Vec<Candidate>,
) -> Result<Vec<DraftTopic>, Error> {
    let saved: Vec<Vec<DraftTopic>> = stage.book.lines(DRAFTS).await?;
    if let Some(drafts) = saved.into_iter().next() {
        return Ok(drafts);
    }
    stage
        .book
        .note(&format!(
            "tree: consolidating {} candidate(s)",
            candidates.len()
        ))
        .await?;
    let candidates = &candidates;
    let (mut drafts, tally) = pool::drive(
        vec![()],
        stage.settings,
        stage.book,
        DRAFTS,
        |()| async move {
            Ok(vec![
                workers::cleanup::consolidate(stage.call, seeds, candidates).await?,
            ])
        },
    )
    .await?;
    stage.incomplete("tree consolidation", tally.failed).await?;
    Ok(drafts.pop().unwrap_or_default())
}

fn seeds(stage: &Stage<'_>) -> Result<Vec<TreeSeed>, Error> {
    let seeded = ProposedTree::propose(stage.snapshot, Vec::new(), stage.guard)?;
    Ok(seeded
        .topics
        .into_iter()
        .map(|topic| TreeSeed {
            name: topic.name,
            aliases: topic.aliases,
            existing: topic.origin == Origin::Existing,
        })
        .collect())
}
