use std::sync::LazyLock;

use config::Environment;
use regress::Regex;
use wire::capture::CaptureMessage;

pub const ENV_VARIABLE: &str = "RIVETHUB_CAPTURE_REDACTION";
const SCAN_LIMIT: usize = 16_000;
const SECRET_KEY_SOURCE: &str = "^(password|passwd|pwd|secret|token|api[_-]?key|apikey|access[_-]?key|private[_-]?key|credential)$";
const SECRET_KEY_PLACEHOLDER: &str = "[REDACTED:secret_key]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replacement {
    Whole,
    FirstGroup,
}

#[derive(Debug)]
struct Builtin {
    name: &'static str,
    regex: Regex,
    replacement: Replacement,
}

const BUILTIN_SOURCES: [(&str, &str, Replacement); 8] = [
    (
        "bearer",
        r"Bearer [A-Za-z0-9\-._~+/=]{16,}",
        Replacement::Whole,
    ),
    (
        "pem_private_key",
        concat!(
            "-----BEGIN [A-Z ]*PRIVATE ",
            r"KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE ",
            "KEY-----"
        ),
        Replacement::Whole,
    ),
    ("aws_access_key", r"AKIA[0-9A-Z]{16}", Replacement::Whole),
    (
        "github_token",
        r"gh[pousr]_[A-Za-z0-9]{30,}",
        Replacement::Whole,
    ),
    (
        "slack_token",
        r"xox[aebprs]-[A-Za-z0-9\-]{10,}",
        Replacement::Whole,
    ),
    (
        "sk_token",
        concat!("sk", r"[_\-][A-Za-z0-9_\-]{20,}"),
        Replacement::Whole,
    ),
    (
        "jwt",
        r"ey[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}",
        Replacement::Whole,
    ),
    (
        "assignment",
        r"\b(api[_-]?key|apikey|access[_-]?key|access[_-]?token|auth[_-]?token|secret|token|authorization|credential)[ \t]*[=:][ \t]*\S+",
        Replacement::FirstGroup,
    ),
];

#[expect(
    clippy::expect_used,
    reason = "the builtin sources are constants; the redaction tests compile every one of them"
)]
static SECRET_KEYS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::with_flags(SECRET_KEY_SOURCE, "i")
        .expect("the secret-key pattern is a constant that compiles")
});

#[expect(
    clippy::expect_used,
    reason = "the builtin sources are constants; the redaction tests compile every one of them"
)]
static BUILTINS: LazyLock<Vec<Builtin>> = LazyLock::new(|| {
    BUILTIN_SOURCES
        .into_iter()
        .map(|(name, source, replacement)| Builtin {
            name,
            regex: Regex::with_flags(source, "i")
                .expect("builtin redaction patterns are constants that compile"),
            replacement,
        })
        .collect()
});

#[derive(Debug)]
pub struct Redaction {
    builtins: &'static [Builtin],
    patterns: Vec<Regex>,
}

#[must_use]
pub fn resolve(
    settings: Option<config::RedactionSettings>,
    env: &Environment,
) -> Option<Redaction> {
    let (builtins_on, patterns) = match settings {
        Some(settings) if settings.enabled => (settings.builtins, settings.patterns),
        _ => {
            if !env_opt_in(env) {
                return None;
            }
            (true, Vec::new())
        }
    };
    if !builtins_on && patterns.is_empty() {
        return None;
    }
    let mut compiled = Vec::new();
    for (index, source) in patterns.iter().enumerate() {
        if unsafe_source(source) {
            tracing::warn!("capture redaction pattern {index} is a nested quantifier; skipped");
            continue;
        }
        match Regex::new(source) {
            Ok(regex) => compiled.push(regex),
            Err(error) => {
                tracing::warn!("capture redaction pattern {index} did not compile: {error}");
            }
        }
    }
    let builtins: &'static [Builtin] = if builtins_on { &BUILTINS } else { &[] };
    Some(Redaction {
        builtins,
        patterns: compiled,
    })
}

fn env_opt_in(env: &Environment) -> bool {
    env.text(ENV_VARIABLE).is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

impl Redaction {
    pub fn redact_message(&self, message: &mut CaptureMessage) -> u64 {
        let mut spans = 0;
        if let Some((redacted, count)) = scan_text(&message.content, self) {
            message.content = redacted;
            spans += count;
        }
        if let Some(result) = &message.tool_result
            && let Some((redacted, count)) = scan_text(result, self)
        {
            message.tool_result = Some(redacted);
            spans += count;
        }
        if let Some(arguments) = &mut message.tool_args {
            arguments.visit_strings_mut(&mut |name, text| {
                let (redacted, count) = apply_spans(text, self);
                spans += count;
                if secret_key(name) {
                    SECRET_KEY_PLACEHOLDER.clone_into(text);
                } else {
                    *text = redacted;
                }
            });
        }
        spans
    }
}

fn apply_spans(text: &str, redaction: &Redaction) -> (String, u64) {
    let mut out = text.to_owned();
    let mut count = 0;
    for builtin in redaction.builtins {
        let placeholder = format!("[REDACTED:{}]", builtin.name);
        let (next, hits) = replace_matches(&out, &builtin.regex, builtin.replacement, &placeholder);
        count += hits;
        out = next;
    }
    for (index, pattern) in redaction.patterns.iter().enumerate() {
        let placeholder = format!("[REDACTED:pattern:{index}]");
        let (next, hits) = replace_matches(&out, pattern, Replacement::Whole, &placeholder);
        count += hits;
        out = next;
    }
    (out, count)
}

fn scan_text(text: &str, redaction: &Redaction) -> Option<(String, u64)> {
    let (head, tail) = text
        .char_indices()
        .nth(SCAN_LIMIT)
        .map_or((text, ""), |(end, _)| text.split_at(end));
    let (redacted_head, count) = apply_spans(head, redaction);
    if redacted_head == head && tail.is_empty() {
        return None;
    }
    Some((format!("{redacted_head}{tail}"), count))
}

fn secret_key(key: &str) -> bool {
    SECRET_KEYS
        .find(key)
        .is_some_and(|found| !found.range().is_empty())
}

fn replace_matches(
    text: &str,
    regex: &Regex,
    replacement: Replacement,
    placeholder: &str,
) -> (String, u64) {
    let mut out = String::new();
    let mut copied = 0;
    let mut count = 0;
    for found in regex.find_iter(text) {
        let range = found.range();
        out.push_str(&text[copied..range.start]);
        match replacement {
            Replacement::Whole => out.push_str(placeholder),
            Replacement::FirstGroup => {
                out.push_str(first_group_or_empty(text, &found));
                out.push_str(placeholder);
            }
        }
        copied = range.end;
        count += 1;
    }
    out.push_str(&text[copied..]);
    (out, count)
}

fn first_group_or_empty<'a>(text: &'a str, found: &regress::Match) -> &'a str {
    found
        .group(1)
        .and_then(|group| text.get(group))
        .unwrap_or("")
}

fn unsafe_source(source: &str) -> bool {
    let characters: Vec<char> = source.chars().collect();
    let mut depth = 0;
    let mut group_start = 0;
    let mut index = 0;
    while index < characters.len() {
        match characters.get(index) {
            Some('\\') => index += 1,
            Some('(') => {
                if depth == 0 {
                    group_start = index;
                }
                depth += 1;
            }
            Some(')') if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    #[expect(
                        clippy::indexing_slicing,
                        reason = "group_start and index bracket one group inside the source"
                    )]
                    let body = &characters[group_start + 1..index];
                    let repeated = characters
                        .get(index + 1)
                        .is_some_and(|next| matches!(next, '+' | '*' | '{' | '?'));
                    if ends_quantified(body) && repeated {
                        return true;
                    }
                }
            }
            _ => {}
        }
        index += 1;
    }
    false
}

fn ends_quantified(body: &[char]) -> bool {
    let trimmed: String = body.iter().filter(|c| !c.is_whitespace()).collect();
    trimmed.ends_with('+')
        || trimmed.ends_with('*')
        || trimmed.ends_with('?')
        || trimmed.ends_with('}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_patterns_are_constants_that_compile() {
        assert_eq!(BUILTINS.len(), BUILTIN_SOURCES.len());
        let canaries = [
            ("bearer", "Bearer abcdefghijklmnop1234"),
            (
                "pem_private_key",
                concat!(
                    "-----BEGIN ",
                    "PRIVATE KEY-----\nx\n-----END ",
                    "PRIVATE KEY-----"
                ),
            ),
            ("aws_access_key", concat!("AKIA", "ABCDEFGHIJKLMNOP")),
            (
                "github_token",
                concat!("ghp_", "abcdefghijklmnopqrstuvwx0123456789"),
            ),
            ("slack_token", concat!("xoxb-", "abcdefghij")),
            ("sk_token", concat!("sk-", "abcdefghijklmnopqrst")),
            (
                "jwt",
                concat!(
                    "eyJhbGciOiJIUzI1NiJ9",
                    ".eyJzdWIiOiIxIn0",
                    ".SflKxwRJSMeKKF2QT4"
                ),
            ),
            ("assignment", "api_key = hunter2"),
        ];
        for (name, canary) in canaries {
            let builtin = BUILTINS
                .iter()
                .find(|builtin| builtin.name == name)
                .unwrap_or_else(|| panic!("builtin {name} is missing"));
            assert!(
                builtin.regex.find(canary).is_some(),
                "builtin {name} does not match its canary"
            );
        }
        assert!(
            SECRET_KEYS.find("api_key").is_some(),
            "the secret-key pattern is missing"
        );
    }

    #[test]
    fn a_capture_group_that_does_not_participate_contributes_nothing() {
        let regex = Regex::new("x(?:-(y))?").expect("the test pattern is a constant that compiles");
        let bare = regex.find("x").expect("the bare form matches");
        assert_eq!(first_group_or_empty("x", &bare), "");
        let grouped = regex.find("x-y").expect("the grouped form matches");
        assert_eq!(first_group_or_empty("x-y", &grouped), "y");
    }
}
