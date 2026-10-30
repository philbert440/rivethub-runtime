mod call;
mod fold;
mod review;
mod tree;

pub use call::ModelCall;
pub use fold::FOLD_SYSTEM_PROMPT;
pub use fold::FoldAnswer;
pub use fold::FoldItem;
pub use fold::fold_batch;
pub use review::REVIEW_SYSTEM_PROMPT;
pub use review::review_tree;
pub use tree::Candidate;
pub use tree::TREE_SYSTEM_PROMPT;
pub use tree::TreeItem;
pub use tree::TreeSeed;
pub use tree::consolidate;
pub use tree::propose_chunk;

use crate::error::WorkerError;

fn json_object(reply: &str) -> Result<&str, WorkerError> {
    let missing = || WorkerError::Cleanup {
        message: "the model answer has no JSON object".to_owned(),
    };
    let start = reply.find('{').ok_or_else(missing)?;
    let end = reply.rfind('}').ok_or_else(missing)?;
    reply.get(start..=end).ok_or_else(missing)
}
