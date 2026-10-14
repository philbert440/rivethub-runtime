use chrono::DateTime;
use chrono::SecondsFormat;
use chrono::Utc;

use crate::error::SqliteError;
use crate::error::TimestampField;

pub(crate) fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stamp {
    Source(DateTime<Utc>),
    CaptureTime,
}

impl From<Option<DateTime<Utc>>> for Stamp {
    fn from(source: Option<DateTime<Utc>>) -> Self {
        match source {
            Some(at) => Self::Source(at),
            None => Self::CaptureTime,
        }
    }
}

impl Stamp {
    pub(crate) const fn at(self, now: DateTime<Utc>) -> DateTime<Utc> {
        match self {
            Self::Source(at) => at,
            Self::CaptureTime => now,
        }
    }
}

pub(crate) fn supplied(raw: Option<&str>, field: TimestampField) -> Result<Stamp, SqliteError> {
    raw.map(|text| {
        DateTime::parse_from_rfc3339(text)
            .map(|parsed| parsed.with_timezone(&Utc))
            .map_err(|source| SqliteError::InvalidTimestamp { field, source })
    })
    .transpose()
    .map(Stamp::from)
}

pub(crate) fn stored(text: &str, column: &'static str) -> Result<DateTime<Utc>, SqliteError> {
    DateTime::parse_from_rfc3339(text)
        .map(|parsed| parsed.with_timezone(&Utc))
        .map_err(|source| SqliteError::StoredTimestamp {
            column,
            value: text.to_owned(),
            source,
        })
}
