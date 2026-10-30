use serde::Deserialize;

use crate::error::WorkerError;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub(super) enum Line {
    Merge {
        merge: String,
        into: String,
        reason: String,
    },
    Move {
        #[serde(rename = "move")]
        topic: String,
        under: Option<String>,
        reason: String,
    },
    Rename {
        rename: String,
        to: String,
        reason: String,
    },
    Close {
        close: String,
        reason: String,
    },
    Delete {
        delete: String,
        reason: String,
    },
    Create {
        create: String,
        name: Option<String>,
        under: Option<String>,
        reason: String,
    },
    Type {
        #[serde(rename = "type")]
        name: String,
        under: Option<String>,
        reason: String,
    },
}

pub(super) fn lines(reply: &str) -> Result<Vec<Line>, WorkerError> {
    let objects: Vec<&str> = reply.lines().filter_map(object).collect();
    let mut parsed = Vec::new();
    for object in &objects {
        let line = match serde_json::from_str::<Line>(object) {
            Ok(line) => Some(line),
            Err(error) => {
                tracing::warn!("tidy: dropped an answer line that is not a proposal: {error}");
                None
            }
        };
        parsed.extend(line);
    }
    if parsed.is_empty() && !objects.is_empty() {
        return Err(WorkerError::TidyAnswer {
            message: "the tidy answer has JSON lines but no proposal".to_owned(),
        });
    }
    Ok(parsed)
}

fn object(line: &str) -> Option<&str> {
    let start = line.find('{')?;
    let end = line.rfind('}')?;
    line.get(start..=end)
}
