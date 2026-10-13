use std::str::FromStr as _;

use chrono::Datelike as _;
use chrono::NaiveDate;
use chrono::Timelike as _;
use thiserror::Error;
use uuid::Uuid;
use wire::harness::HarnessId;
use wire::session_id::SessionId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NativeSession {
    harness: HarnessId,
    native: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdentityError {
    #[error("{harness} native session id {native:?} does not match the recorded shape")]
    Shape { harness: HarnessId, native: String },
    #[error("{harness} native session id {native:?} is not a version-7 uuid")]
    UuidVersion { harness: HarnessId, native: String },
    #[error("session id {canonical:?} is not a harness-prefixed native session id")]
    Canonical { canonical: String },
}

impl NativeSession {
    pub fn parse(harness: HarnessId, native: &str) -> Result<Self, IdentityError> {
        matches_shape(harness, native)?;
        Ok(Self {
            harness,
            native: native.to_owned(),
        })
    }

    #[must_use]
    pub fn harness(&self) -> HarnessId {
        self.harness
    }

    #[must_use]
    pub fn native(&self) -> &str {
        &self.native
    }

    #[must_use]
    pub fn canonical(&self) -> SessionId {
        SessionId::new(self.harness, &self.native)
    }
}

pub fn from_canonical(session: &SessionId) -> Result<NativeSession, IdentityError> {
    let canonical = session.as_str();
    let Some((prefix, native)) = canonical.split_once(':') else {
        return Err(IdentityError::Canonical {
            canonical: canonical.to_owned(),
        });
    };
    let Ok(harness) = HarnessId::from_str(prefix) else {
        return Err(IdentityError::Canonical {
            canonical: canonical.to_owned(),
        });
    };
    NativeSession::parse(harness, native)
}

fn matches_shape(harness: HarnessId, native: &str) -> Result<(), IdentityError> {
    let shaped = match harness {
        HarnessId::ClaudeCode
        | HarnessId::Pi
        | HarnessId::QwenCode
        | HarnessId::Cursor
        | HarnessId::Cowork
        | HarnessId::Codex
        | HarnessId::GrokBuild => is_uuid(native),
        HarnessId::Hermes => is_hermes(native),
        HarnessId::KimiCode => is_kimi(native),
        HarnessId::Opencode => is_opencode(native),
    };
    if !shaped {
        return Err(IdentityError::Shape {
            harness,
            native: native.to_owned(),
        });
    }
    if harness == HarnessId::GrokBuild
        && !Uuid::parse_str(native).is_ok_and(|uuid| uuid.get_version_num() == 7)
    {
        return Err(IdentityError::UuidVersion {
            harness,
            native: native.to_owned(),
        });
    }
    Ok(())
}

fn is_uuid(native: &str) -> bool {
    Uuid::parse_str(native).is_ok_and(|uuid| uuid.hyphenated().to_string() == native)
}

fn is_hermes(native: &str) -> bool {
    let mut fields = native.split('_');
    let Some(day) = fields.next() else {
        return false;
    };
    let Some(clock) = fields.next() else {
        return false;
    };
    let Some(tag) = fields.next() else {
        return false;
    };
    fields.next().is_none() && is_hermes_stamp(day, clock) && is_hermes_tag(tag)
}

fn is_hermes_stamp(day: &str, clock: &str) -> bool {
    let Ok(date) = NaiveDate::parse_from_str(day, "%Y%m%d") else {
        return false;
    };
    let Ok(time) = chrono::NaiveTime::parse_from_str(clock, "%H%M%S") else {
        return false;
    };
    date.year() > 0 && time.hour() < 24 && time.minute() < 60 && time.second() < 60
}

fn is_hermes_tag(tag: &str) -> bool {
    tag.len() == 6 && tag.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_kimi(native: &str) -> bool {
    native.strip_prefix("session_").is_some_and(is_uuid)
}

fn is_opencode(native: &str) -> bool {
    native
        .strip_prefix("ses_")
        .is_some_and(|rest| rest.len() >= 20 && rest.chars().all(|c| c.is_ascii_alphanumeric()))
}
