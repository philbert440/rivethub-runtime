pub(super) fn fts_match(text: &str) -> Option<String> {
    join_tokens(text, " AND ")
}

pub(crate) fn fts_or(text: &str) -> Option<String> {
    join_tokens(text, " OR ")
}

fn join_tokens(text: &str, separator: &str) -> Option<String> {
    let tokens: Vec<String> = text.split_whitespace().filter_map(fts_token).collect();
    if tokens.is_empty() {
        None
    } else {
        Some(tokens.join(separator))
    }
}

fn fts_token(raw: &str) -> Option<String> {
    let cleaned: String = raw.chars().filter(|ch| !stripped(*ch)).collect();
    if cleaned.is_empty() {
        None
    } else {
        Some(format!("\"{cleaned}\""))
    }
}

const fn stripped(ch: char) -> bool {
    matches!(ch, '"' | '\'' | '^' | '~' | '*' | '(' | ')' | ':')
}

#[cfg(test)]
mod tests {
    use super::fts_match;

    #[test]
    fn recorded_fts_match_strings() {
        assert_eq!(fts_match("deploy").as_deref(), Some("\"deploy\""));
        assert_eq!(
            fts_match("deploy release").as_deref(),
            Some("\"deploy\" AND \"release\"")
        );
        assert_eq!(
            fts_match("\"unbalanced AND (").as_deref(),
            Some("\"unbalanced\" AND \"AND\"")
        );
        assert_eq!(fts_match("*** ^~ ()::''"), None);
        assert_eq!(
            fts_match("api.example.com:8443").as_deref(),
            Some("\"api.example.com8443\"")
        );
        assert_eq!(fts_match("zzzzqqq").as_deref(), Some("\"zzzzqqq\""));
        assert_eq!(fts_match("staging").as_deref(), Some("\"staging\""));
        assert_eq!(fts_match("rollback").as_deref(), Some("\"rollback\""));
    }
}
