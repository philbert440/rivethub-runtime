pub const SUMMARY_TITLE_MAX_CHARS: usize = 80;

const UNTITLED: &str = "untitled";

#[must_use]
pub fn summary_title(content: &str) -> Option<String> {
    let text = strip_markup(content.trim());
    if text.is_empty() {
        return None;
    }
    let sentence = first_sentence(text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let sentence = strip_markup(&sentence);
    if sentence.is_empty() || sentence.eq_ignore_ascii_case(UNTITLED) {
        return None;
    }
    Some(capped(sentence))
}

fn strip_markup(text: &str) -> &str {
    text.trim_start_matches(|c: char| matches!(c, '#' | '>' | '*' | '_' | '-') || c.is_whitespace())
        .trim_end()
}

fn first_sentence(text: &str) -> String {
    let mut sentence = String::new();
    let mut chars = text.chars().peekable();
    let mut leading = true;
    while let Some(current) = chars.next() {
        sentence.push(current);
        let closes = !leading
            && matches!(current, '.' | '!' | '?')
            && chars.peek().is_none_or(|next| next.is_whitespace());
        if closes {
            break;
        }
        leading = false;
    }
    sentence
}

fn capped(sentence: &str) -> String {
    if sentence.chars().count() <= SUMMARY_TITLE_MAX_CHARS {
        return sentence.to_owned();
    }
    let head: String = sentence.chars().take(SUMMARY_TITLE_MAX_CHARS - 1).collect();
    let word = match head.rsplit_once(' ') {
        Some((before, _)) => before,
        None => head.as_str(),
    };
    let cut = if word.is_empty() || word.chars().count() < SUMMARY_TITLE_MAX_CHARS / 2 {
        head.as_str()
    } else {
        word
    };
    format!(
        "{}\u{2026}",
        cut.trim_end_matches(['.', ',', ';', ':', ' '])
    )
}

#[cfg(test)]
mod tests {
    use super::SUMMARY_TITLE_MAX_CHARS;
    use super::summary_title;

    #[test]
    fn the_first_sentence_becomes_the_title() {
        assert_eq!(
            summary_title("Fixed the queue consumer. Also rewrote the retry budget.").as_deref(),
            Some("Fixed the queue consumer.")
        );
        assert_eq!(
            summary_title("Is the cache warm? It was cold.").as_deref(),
            Some("Is the cache warm?")
        );
        assert_eq!(
            summary_title("Version 1.2 shipped\nacross   the fleet").as_deref(),
            Some("Version 1.2 shipped across the fleet")
        );
    }

    #[test]
    fn markdown_noise_is_stripped_and_empty_leaves_give_no_title() {
        assert_eq!(
            summary_title("### Fixed the queue consumer.").as_deref(),
            Some("Fixed the queue consumer.")
        );
        assert_eq!(
            summary_title("> **Moved the build** to the new host.").as_deref(),
            Some("Moved the build** to the new host.")
        );
        assert_eq!(summary_title(""), None);
        assert_eq!(summary_title("   \n"), None);
        assert_eq!(summary_title("### "), None);
        assert_eq!(summary_title("- - -"), None);
        assert_eq!(summary_title("Untitled"), None);
    }

    #[test]
    fn long_sentences_are_cut_on_a_word_boundary_with_an_ellipsis() {
        let long = "word ".repeat(40);
        let title = summary_title(&long).expect("title");
        assert!(title.chars().count() <= SUMMARY_TITLE_MAX_CHARS);
        assert!(title.ends_with("word\u{2026}"), "{title}");
        let solid = format!("{} more words that should be cut.", "A".repeat(120));
        let title = summary_title(&solid).expect("title");
        assert_eq!(title.chars().count(), SUMMARY_TITLE_MAX_CHARS);
        assert_eq!(title, format!("{}\u{2026}", "A".repeat(79)));
    }

    #[test]
    fn the_cap_counts_characters_not_bytes() {
        let wide = "\u{e9}".repeat(100);
        let title = summary_title(&wide).expect("title");
        assert_eq!(title.chars().count(), SUMMARY_TITLE_MAX_CHARS);
        let exact = "\u{e9}".repeat(SUMMARY_TITLE_MAX_CHARS);
        assert_eq!(summary_title(&exact).as_deref(), Some(exact.as_str()));
    }
}
