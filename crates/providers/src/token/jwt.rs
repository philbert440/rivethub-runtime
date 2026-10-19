use std::time::Duration;
use std::time::SystemTime;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use thiserror::Error;

const EXPIRY_MARGIN: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub(super) enum ExpiryError {
    #[error("the bearer is not three dot-separated segments")]
    Shape,
    #[error("the bearer payload is not base64url: {source}")]
    Encoding {
        #[source]
        source: base64::DecodeError,
    },
    #[error("the bearer payload carries no integer exp claim: {source}")]
    Claims {
        #[source]
        source: serde_json::Error,
    },
    #[error("the system clock reads before the Unix epoch: {source}")]
    Clock {
        #[source]
        source: std::time::SystemTimeError,
    },
}

#[derive(Deserialize)]
struct Claims {
    exp: u64,
}

pub(super) fn lifetime(token: &str, now: SystemTime) -> Result<Duration, ExpiryError> {
    let mut segments = token.split('.');
    let (Some(_), Some(payload), Some(_), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return Err(ExpiryError::Shape);
    };
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|source| ExpiryError::Encoding { source })?;
    let claims: Claims =
        serde_json::from_slice(&bytes).map_err(|source| ExpiryError::Claims { source })?;
    let elapsed = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|source| ExpiryError::Clock { source })?;
    Ok(Duration::from_secs(claims.exp)
        .saturating_sub(elapsed)
        .saturating_sub(EXPIRY_MARGIN))
}
