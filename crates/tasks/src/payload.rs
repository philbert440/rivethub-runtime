use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskJobPayload {
    pub task_id: String,
}

pub(crate) fn as_text<T: Serialize>(value: &T) -> Result<String, Error> {
    let encoded =
        serde_json::to_string(value).map_err(|source| Error::Stored(source.to_string()))?;
    serde_json::from_str(&encoded).map_err(|source| Error::Stored(source.to_string()))
}

pub(crate) fn from_text<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    let encoded =
        serde_json::to_string(text).map_err(|source| Error::Stored(source.to_string()))?;
    serde_json::from_str(&encoded).map_err(|source| Error::Stored(source.to_string()))
}
