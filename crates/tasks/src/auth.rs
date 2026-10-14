use crate::error::Error;

const UNSET_TOKEN: &str = "tasks-api-token-unset";

pub(crate) fn authorize(configured: Option<&str>, presented: Option<&str>) -> Result<(), Error> {
    let configured_token = match configured {
        Some(token) if !token.is_empty() => Some(token),
        Some(_) | None => None,
    };
    let expected = match configured_token {
        Some(token) => token,
        None => UNSET_TOKEN,
    };
    #[expect(
        clippy::manual_unwrap_or_default,
        reason = "an absent presented token is compared as empty bytes; unwrap_or_default is rejected as a silent fallback"
    )]
    let given = match presented {
        Some(token) => token,
        None => "",
    };
    let matched = constant_time_eq(expected.as_bytes(), given.as_bytes());
    if configured_token.is_some() && matched {
        Ok(())
    } else {
        Err(Error::Unauthorized)
    }
}

pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = u8::from(left.len() != right.len());
    let width = left.len().max(right.len());
    let mut index = 0;
    while index < width {
        diff |= byte_at(left, index) ^ byte_at(right, index);
        index += 1;
    }
    diff == 0
}

fn byte_at(bytes: &[u8], index: usize) -> u8 {
    match bytes.get(index) {
        Some(byte) => *byte,
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::authorize;
    use super::constant_time_eq;

    #[test]
    fn a_configured_token_authorizes_only_the_same_bytes() {
        assert!(authorize(Some("secret"), Some("secret")).is_ok());
        assert!(authorize(Some("secret"), Some("other")).is_err());
        assert!(authorize(Some("secret"), Some("secret-extra")).is_err());
        assert!(authorize(Some("secret"), None).is_err());
        assert!(authorize(None, Some("secret")).is_err());
        assert!(authorize(Some(""), Some("")).is_err());
        assert!(authorize(Some(""), None).is_err());
    }

    #[test]
    fn constant_time_compare_requires_equal_bytes() {
        assert!(constant_time_eq(b"ab", b"ab"));
        assert!(!constant_time_eq(b"ab", b"ac"));
        assert!(!constant_time_eq(b"ab", b"a"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }
}
