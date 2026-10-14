pub const DEFAULT_EMBED_QUERY_INSTRUCTION: &str =
    "Instruct: Given a search query, retrieve relevant passages that answer the query\nQuery: ";
const EMBED_INPUT_MAX: usize = 8_000;

#[must_use]
pub fn compose_query_input(instruction: &str, text: &str) -> String {
    format!("{instruction}{}", normalize_query_text(text))
        .chars()
        .take(EMBED_INPUT_MAX)
        .collect()
}

fn normalize_query_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_EMBED_QUERY_INSTRUCTION;
    use super::EMBED_INPUT_MAX;
    use super::compose_query_input;
    use super::normalize_query_text;

    #[test]
    fn normalisation_trims_and_collapses_every_whitespace_run() {
        assert_eq!(
            normalize_query_text("  deploy   the\trelease\nnow  "),
            "deploy the release now"
        );
        assert_eq!(
            normalize_query_text("\u{3000}全角\u{a0}space\r\n"),
            "全角 space"
        );
    }

    #[test]
    fn normalisation_of_blank_text_is_empty() {
        assert_eq!(normalize_query_text(""), "");
        assert_eq!(normalize_query_text(" \t\n "), "");
    }

    #[test]
    fn normalisation_keeps_single_spaced_text_unchanged() {
        assert_eq!(normalize_query_text("already clean"), "already clean");
    }

    #[test]
    fn the_instruction_prefixes_the_normalised_text() {
        assert_eq!(
            compose_query_input("prefix ", "  spaced   out "),
            "prefix spaced out"
        );
        assert_eq!(compose_query_input("", "raw text"), "raw text");
        assert_eq!(
            compose_query_input(DEFAULT_EMBED_QUERY_INSTRUCTION, "where is the ca"),
            "Instruct: Given a search query, retrieve relevant passages that answer the query\nQuery: where is the ca"
        );
    }

    #[test]
    fn the_instruction_keeps_its_own_whitespace() {
        assert_eq!(compose_query_input("a\n  b: ", "c"), "a\n  b: c");
    }

    #[test]
    fn input_under_the_cap_is_kept_whole() {
        let text = "x".repeat(EMBED_INPUT_MAX - 2);
        assert_eq!(compose_query_input("> ", &text), format!("> {text}"));
    }

    #[test]
    fn long_input_is_cut_at_the_cap_in_chars() {
        let composed = compose_query_input(DEFAULT_EMBED_QUERY_INSTRUCTION, &"word ".repeat(4_000));
        assert_eq!(composed.chars().count(), EMBED_INPUT_MAX);
        assert!(composed.starts_with(DEFAULT_EMBED_QUERY_INSTRUCTION));
    }

    #[test]
    fn the_cap_counts_multibyte_and_astral_chars_as_one() {
        let text = "é😀".repeat(EMBED_INPUT_MAX);
        let composed = compose_query_input("", &text);
        assert_eq!(composed.chars().count(), EMBED_INPUT_MAX);
        assert_eq!(composed, "é😀".repeat(EMBED_INPUT_MAX / 2));
    }

    #[test]
    fn the_cap_applies_to_the_instruction_too() {
        let instruction = "i".repeat(EMBED_INPUT_MAX + 5);
        let composed = compose_query_input(&instruction, "lost");
        assert_eq!(composed, "i".repeat(EMBED_INPUT_MAX));
    }
}
