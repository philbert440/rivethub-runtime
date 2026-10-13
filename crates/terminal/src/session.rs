use serde::Deserialize;
use serde::Serialize;
use std::fmt;
use std::str::FromStr;

use crate::Error;

const SESSION_KEY_PATTERN_END: usize = 120;
const ROSTER_KEY_PATTERN_END: usize = 32;

fn is_session_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'.' | b'-')
}

fn is_roster_key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'.' || byte == b'-'
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct SessionKey(String);

impl SessionKey {
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        let bytes = value.as_bytes();
        if bytes.is_empty() || bytes.len() > SESSION_KEY_PATTERN_END {
            return Err(Error::InvalidSessionKey(value));
        }
        if !bytes.iter().all(|byte| is_session_key_byte(*byte)) {
            return Err(Error::InvalidSessionKey(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<SessionKey> for String {
    fn from(value: SessionKey) -> Self {
        value.0
    }
}

impl TryFrom<String> for SessionKey {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for SessionKey {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RosterKey(String);

impl RosterKey {
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        let bytes = value.as_bytes();
        if bytes.is_empty() || bytes.len() > ROSTER_KEY_PATTERN_END {
            return Err(Error::InvalidRosterKey(value));
        }
        if !bytes.iter().all(|byte| is_roster_key_byte(*byte)) {
            return Err(Error::InvalidRosterKey(value));
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RosterKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<RosterKey> for String {
    fn from(value: RosterKey) -> Self {
        value.0
    }
}

impl TryFrom<String> for RosterKey {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for RosterKey {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value.to_owned())
    }
}
