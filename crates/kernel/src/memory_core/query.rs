pub const DEFAULT_EMBED_QUERY_INSTRUCTION: &str =
    "Instruct: Given a search query, retrieve relevant passages that answer the query\nQuery: ";
pub const EMBED_INPUT_MAX: usize = 8_000;

#[must_use]
pub fn normalize_query_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[must_use]
pub fn apply_embed_query_instruction(instruction: &str, text: &str) -> String {
    truncate_chars(&format!("{instruction}{text}"), EMBED_INPUT_MAX)
}

#[must_use]
pub fn compose_query_input(instruction: &str, text: &str) -> String {
    apply_embed_query_instruction(instruction, &normalize_query_text(text))
}

fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() > max_chars {
        input.chars().take(max_chars).collect()
    } else {
        input.to_owned()
    }
}
