use std::collections::BTreeMap;

use kernel::filing::Answer;
use kernel::topics::TopicName;
use serde::Deserialize;

use super::prompt::Tree;
use super::prompt::batch_ref;
use crate::error::WorkerError;

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Reply {
    File {
        file: String,
    },
    Suggest {
        suggest: String,
        #[serde(default)]
        under: Option<String>,
    },
    Nothing {
        none: bool,
    },
}

pub(super) fn answer(reply: &str, tree: &Tree) -> Result<Answer, WorkerError> {
    let object = json_object(reply)?;
    let parsed: Reply = serde_json::from_str(object).map_err(|source| WorkerError::Filing {
        message: format!("the filing answer is not a filing object: {source}"),
    })?;
    settled(parsed, tree)
}

pub(super) fn batch(
    reply: &str,
    tree: &Tree,
    count: usize,
) -> Result<Vec<Result<Answer, WorkerError>>, WorkerError> {
    let object = json_object(reply)?;
    let mut parsed: BTreeMap<String, Reply> =
        serde_json::from_str(object).map_err(|source| WorkerError::Filing {
            message: format!("the batch filing answer is not a map of filing objects: {source}"),
        })?;
    Ok((0..count)
        .map(|index| {
            let reference = batch_ref(index);
            match parsed.remove(&reference) {
                Some(reply) => settled(reply, tree),
                None => Err(WorkerError::Filing {
                    message: format!("the batch filing answer has nothing for {reference}"),
                }),
            }
        })
        .collect())
}

fn settled(parsed: Reply, tree: &Tree) -> Result<Answer, WorkerError> {
    match parsed {
        Reply::File { file } => Ok(Answer::File(known(tree, &file)?)),
        Reply::Suggest { suggest, under } => Ok(Answer::Suggest {
            name: TopicName::new(&suggest)?,
            under: under
                .as_deref()
                .map(|under| known(tree, under))
                .transpose()?,
        }),
        Reply::Nothing { none: true } => Ok(Answer::Nothing),
        Reply::Nothing { none: false } => Err(WorkerError::Filing {
            message: "the filing answer says none: false".to_owned(),
        }),
    }
}

fn known(tree: &Tree, reference: &str) -> Result<kernel::topics::TopicId, WorkerError> {
    tree.topic(reference).ok_or_else(|| WorkerError::Filing {
        message: format!("the filing answer names unknown topic {reference}"),
    })
}

fn json_object(reply: &str) -> Result<&str, WorkerError> {
    let missing = || WorkerError::Filing {
        message: "the filing answer has no JSON object".to_owned(),
    };
    let start = reply.find('{').ok_or_else(missing)?;
    let end = reply.rfind('}').ok_or_else(missing)?;
    reply.get(start..=end).ok_or_else(missing)
}
