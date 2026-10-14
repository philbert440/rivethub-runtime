use serde::de::DeserializeOwned;

use crate::error::Error;

pub(crate) fn from_text<T: DeserializeOwned>(text: &str) -> Result<T, Error> {
    let encoded =
        serde_json::to_string(text).map_err(|source| Error::Stored(source.to_string()))?;
    serde_json::from_str(&encoded).map_err(|source| Error::Stored(source.to_string()))
}
