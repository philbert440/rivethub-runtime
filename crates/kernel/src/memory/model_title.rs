use schemars::JsonSchema;
use serde::Deserialize;

use super::ConversationHead;
use super::names_session;
use super::needs_title;
use super::weak_title;
use crate::filing::Filable;

pub const MAX_TITLE_WORDS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TitleAnswer {
    pub title: String,
}

#[must_use]
pub fn title_schema() -> schemars::Schema {
    schemars::schema_for!(TitleAnswer)
}

#[must_use]
pub fn wants_model_title(head: &ConversationHead) -> bool {
    needs_title(head.title.as_deref()) && Filable::of(head) != Filable::Never
}

#[must_use]
pub fn fit_title(answer: &str) -> Option<String> {
    let words: Vec<&str> = answer
        .trim()
        .trim_matches(['"', '\'', '`'])
        .split_whitespace()
        .collect();
    let title = words.join(" ").trim_end_matches('.').to_owned();
    let fits = words.len() <= MAX_TITLE_WORDS
        && title.chars().any(char::is_alphabetic)
        && names_session(&title)
        && !weak_title(&title);
    fits.then_some(title)
}

#[cfg(test)]
mod tests {
    use super::fit_title;

    #[test]
    fn a_short_specific_answer_is_kept_and_tidied() {
        assert_eq!(
            fit_title("  \"Rotate the  AcmeKit signing keys.\" ").as_deref(),
            Some("Rotate the AcmeKit signing keys")
        );
        assert_eq!(
            fit_title("Review PR #42 retry fixes").as_deref(),
            Some("Review PR #42 retry fixes")
        );
    }

    #[test]
    fn long_placeholder_and_weak_answers_are_refused() {
        for answer in [
            "",
            "...",
            "one two three four five six seven eight nine",
            "Grok Build session",
            "Kimi Code session start",
            "Session task:00000000-0000-4000-8000-000000000001",
            "Deploy at 2026-01-02T03:04Z",
        ] {
            assert_eq!(fit_title(answer), None, "{answer:?}");
        }
    }
}
