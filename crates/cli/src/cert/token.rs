use crate::Error;
use base64::Engine;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::path::Path;

use super::validate::Name;

pub(super) struct Enrollment {
    pub(super) token: String,
    pub(super) names: Vec<Name>,
}

pub(super) fn read(path: &Path) -> Result<Enrollment, Error> {
    let text = super::files::read_external(path)?;
    let token = text.trim();
    if token.is_empty() || token.bytes().any(|byte| byte.is_ascii_whitespace()) {
        return Err(invalid());
    }
    Ok(Enrollment {
        names: claims(token)?,
        token: token.to_owned(),
    })
}

fn claims(token: &str) -> Result<Vec<Name>, Error> {
    let mut segments = token.split('.');
    let header = segments.next().ok_or_else(invalid)?;
    let payload = segments.next().ok_or_else(invalid)?;
    let signature = segments.next().ok_or_else(invalid)?;
    if segments.next().is_some() || header.is_empty() || payload.is_empty() || signature.is_empty()
    {
        return Err(invalid());
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid())?;
    let parsed: Claims = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    names(&parsed.sans)
}

fn names(values: &[String]) -> Result<Vec<Name>, Error> {
    if values.is_empty() {
        return Err(Error::PkiInput(
            "enrollment token does not pin a subject alternative name",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut names = Vec::with_capacity(values.len());
    for value in values {
        let name = super::validate::name(value)?;
        if !seen.insert(name.clone()) {
            return Err(Error::PkiInput(
                "enrollment token repeats a subject alternative name",
            ));
        }
        names.push(name);
    }
    Ok(names)
}

fn invalid() -> Error {
    Error::PkiInput("enrollment token is not a step-ca one-time token")
}

#[derive(Deserialize)]
struct Claims {
    sans: Vec<String>,
}
