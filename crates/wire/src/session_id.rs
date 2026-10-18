use crate::harness::HarnessId;
use base64::Engine;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};
use std::str::FromStr;
use thiserror::Error;

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
pub struct SessionId(String);

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
        Self(format!("{harness}:{native}"))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
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
        HarnessId::from_str(prefix)
            .map_err(|_| SessionIdError::UnknownHarness(prefix.to_string()))?;
        Ok(Self(value.to_string()))
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
        value.0
    }
}

const SEGMENT: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::RequireNone)
        .with_decode_allow_trailing_bits(true),
);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SessionRef {
    Canonical(SessionId),
    Bare(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedSegment {
    pub requested: String,
    pub session: SessionRef,
}

impl DecodedSegment {
    #[must_use]
    pub fn redirected_to(&self, resolved: &SessionId) -> Option<SessionId> {
        (self.requested != resolved.as_str()).then(|| resolved.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid session id: {segment}")]
pub struct SegmentError {
    pub segment: String,
}

#[must_use]
pub fn encode_segment(session: &SessionId) -> String {
    SEGMENT.encode(session.as_str())
}

pub fn decode_segment(segment: &str) -> Result<DecodedSegment, SegmentError> {
    let decoded = percent_decode_str(segment).decode_utf8();
    #[cfg_attr(
        dylint_lib = "rr_lints",
        expect(
            fallback,
            reason = "PATTERNS FALLBACK: segment decode keeps the raw segment when percent-decoding fails"
        )
    )]
    let text: &str = match &decoded {
        Ok(text) => text,
        Err(_) => segment,
    };
    if let Some(canonical) = canonical_from_base64(text) {
        return Ok(DecodedSegment {
            session: SessionRef::Canonical(collapse_path_fallback(&canonical)),
            requested: canonical.0,
        });
    }
    if is_uuid(text) {
        return Ok(DecodedSegment {
            requested: text.to_owned(),
            session: SessionRef::Bare(text.to_owned()),
        });
    }
    Err(SegmentError {
        segment: segment.to_owned(),
    })
}

fn canonical_from_base64(text: &str) -> Option<SessionId> {
    if !text.is_empty()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        && let Ok(bytes) = SEGMENT.decode(text)
        && let Ok(decoded) = String::from_utf8(bytes)
        && let Ok(session) = decoded.parse()
    {
        return Some(session);
    }
    None
}

fn collapse_path_fallback(session: &SessionId) -> SessionId {
    if let Some((harness, native)) = session.as_str().split_once(':')
        && let Some((_, tail)) = native.rsplit_once('/')
        && is_uuid(tail)
    {
        return SessionId(format!("{harness}:{tail}"));
    }
    session.clone()
}

fn is_uuid(text: &str) -> bool {
    text.len() == 36
        && text.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_hexdigit(),
        })
}
