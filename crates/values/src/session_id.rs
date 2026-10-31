use crate::HarnessId;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::str::FromStr;
use thiserror::Error;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(Canonical);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Canonical {
    text: String,
    harness: HarnessId,
    native: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SessionIdError {
    #[error("SessionId has leading/trailing whitespace")]
    Whitespace,
    #[error("SessionId is missing a harness-id prefix")]
    MissingPrefix,
    #[error("SessionId has an empty native session id")]
    EmptyNative,
    #[error("unknown harness id: {0}")]
    UnknownHarness(String),
}

impl SessionId {
    #[must_use]
    pub fn new(harness: HarnessId, native: &str) -> Self {
        Self(Canonical {
            text: format!("{harness}:{native}"),
            harness,
            native: native.to_owned(),
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0.text
    }

    #[must_use]
    pub fn harness(&self) -> HarnessId {
        self.0.harness
    }

    #[must_use]
    pub fn native(&self) -> &str {
        &self.0.native
    }
}

impl Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.text)
    }
}

impl FromStr for SessionId {
    type Err = SessionIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.starts_with(char::is_whitespace) || value.ends_with(char::is_whitespace) {
            return Err(SessionIdError::Whitespace);
        }
        let Some((prefix, native)) = value.split_once(':') else {
            return Err(SessionIdError::MissingPrefix);
        };
        if native.is_empty() {
            return Err(SessionIdError::EmptyNative);
        }
        let harness = HarnessId::from_str(prefix)
            .map_err(|_| SessionIdError::UnknownHarness(prefix.to_string()))?;
        Ok(Self(Canonical {
            text: value.to_string(),
            harness,
            native: native.to_string(),
        }))
    }
}

impl TryFrom<String> for SessionId {
    type Error = SessionIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::from_str(&value)
    }
}

impl From<SessionId> for String {
    fn from(value: SessionId) -> Self {
        value.0.text
    }
}
