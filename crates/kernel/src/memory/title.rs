pub const SUMMARY_TITLE_MAX_CHARS: usize = 80;

const UNTITLED: &str = "untitled";
const SESSION_SUFFIX: &str = " session";
const IMPORTED_SUFFIX: &str = " session (imported)";
const AGENTS_HEADER: &str = "# AGENTS.md instructions";

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
enum PlaceholderHarness {
    ClaudeCode,
    Codex,
    GrokBuild,
    KimiCode,
    Dsh,
    Opencode,
    Pi,
    QwenCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum::EnumString)]
#[strum(serialize_all = "kebab-case", ascii_case_insensitive)]
enum FramingWord {
    And,
    Batch,
    Context,
    Conversation,
    Covered,
    Initialization,
    Metadata,
    Overview,
    Session,
    Start,
    Startup,
    Topics,
}

enum Lead<'a> {
    Words,
    Block(&'a str),
    Unclosed,
}

#[must_use]
pub fn names_session(title: &str) -> bool {
    let title = title.trim();
    !title.is_empty() && !placeholder(title) && matches!(lead(title), Lead::Words)
}

#[must_use]
pub fn spoken_title(text: &str) -> Option<&str> {
    let mut rest = text.trim();
    loop {
        match lead(rest) {
            Lead::Words => return names_session(rest).then_some(rest),
            Lead::Block(after) => rest = after.trim(),
            Lead::Unclosed => return None,
        }
    }
}

#[must_use]
pub fn summary_title(content: &str) -> Option<String> {
    let mut lines = content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let first = lines.next()?;
    if heading(first).is_none() {
        return prose_title(content);
    }
    std::iter::once(first)
        .chain(lines)
        .filter_map(heading)
        .map(|text| collapsed(&first_sentence(text)))
        .find(|text| !framing(text) && titles(text))
        .map(|text| capped(&text))
}

fn placeholder(title: &str) -> bool {
    let folded = collapsed(title).to_lowercase();
    if let Some(name) = folded.strip_suffix(IMPORTED_SUFFIX) {
        return !name.is_empty() && !name.contains(' ');
    }
    folded
        .strip_suffix(SESSION_SUFFIX)
        .is_some_and(|name| name.replace(' ', "-").parse::<PlaceholderHarness>().is_ok())
}

fn lead(text: &str) -> Lead<'_> {
    if let Some(after) = text.strip_prefix(AGENTS_HEADER) {
        return Lead::Block(after.trim_start_matches(|c: char| c != '\n' && c != '<'));
    }
    let Some(tag) = text.strip_prefix('<') else {
        return Lead::Words;
    };
    let name_end = tag
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | ':' | '.')))
        .unwrap_or(tag.len());
    let (name, after) = tag.split_at(name_end);
    let opens = name.starts_with(|c: char| c.is_ascii_alphabetic())
        && after.starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace());
    if !opens {
        return Lead::Words;
    }
    let Some((attributes, body)) = after.split_once('>') else {
        return Lead::Unclosed;
    };
    if attributes.ends_with('/') {
        return Lead::Block(body);
    }
    match body.split_once(&format!("</{name}>")) {
        Some((_, rest)) => Lead::Block(rest),
        None => Lead::Unclosed,
    }
}

fn heading(line: &str) -> Option<&str> {
    if let Some(marks) = line.strip_prefix('#') {
        let text = marks.trim_start_matches('#');
        if !text.is_empty() && !text.starts_with(char::is_whitespace) {
            return None;
        }
        return Some(text.trim().trim_matches('*').trim());
    }
    let item = ["- ", "* ", "> "]
        .iter()
        .find_map(|marker| line.strip_prefix(marker))
        .unwrap_or(line);
    let (inner, rest) = item.strip_prefix("**")?.split_once("**")?;
    let rest = rest.trim_start();
    let label = rest.is_empty() || rest.starts_with([':', '-', '\u{2013}', '\u{2014}']);
    label.then(|| inner.trim().trim_end_matches(':').trim_end())
}

fn framing(heading: &str) -> bool {
    let folded = heading.to_lowercase();
    folded.contains("recommended plugins")
        || folded.contains("agents.md")
        || folded
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .all(|word| word.parse::<FramingWord>().is_ok())
}

fn titles(text: &str) -> bool {
    text.chars().any(char::is_alphabetic)
        && !text.eq_ignore_ascii_case(UNTITLED)
        && names_session(text)
}

fn prose_title(content: &str) -> Option<String> {
    let text = strip_markup(content.trim());
    let sentence = strip_markup(&collapsed(&first_sentence(text)).replace("**", "")).to_owned();
    titles(&sentence).then(|| capped(&sentence))
}

fn collapsed(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
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
    use super::names_session;
    use super::spoken_title;
    use super::summary_title;

    #[test]
    fn placeholders_blanks_and_injected_blocks_do_not_name_a_session() {
        for title in [
            "",
            "  \n",
            "Grok Build session",
            "kimi code  session",
            "Claude Code session",
            "dsh session",
            "opus session (imported)",
            "<recommended_plugins> Here is a list of plugins that are",
            "# AGENTS.md instructions <INSTRUCTIONS> # CODEX.md",
            "<environment_context><cwd>/w</cwd></environment_context>",
        ] {
            assert!(!names_session(title), "{title:?}");
        }
        for title in [
            "# Plan review for the cache rewrite",
            "<3 the new build",
            "<= 5 retries on the queue",
            "Debug session",
            "Fix the session store",
            "USER: Heartbeat check. Read the notes file",
        ] {
            assert!(names_session(title), "{title:?}");
        }
    }

    #[test]
    fn the_person_s_words_follow_injected_blocks() {
        assert_eq!(
            spoken_title(
                "<user_instructions>be brief</user_instructions>\n<env a=\"1\"/>\n  fix the cache"
            ),
            Some("fix the cache")
        );
        assert_eq!(
            spoken_title(
                "# AGENTS.md instructions for /tmp/x\n\n<INSTRUCTIONS>\nrules\n</INSTRUCTIONS>\nship it"
            ),
            Some("ship it")
        );
        assert_eq!(
            spoken_title("<command-name>/model</command-name>\n<command-args></command-args>"),
            None
        );
        assert_eq!(
            spoken_title("<recommended_plugins> Here is a list of"),
            None
        );
        assert_eq!(spoken_title("Grok Build session"), None);
        assert_eq!(
            spoken_title("  rename the queue "),
            Some("rename the queue")
        );
    }

    #[test]
    fn a_leading_heading_titles_the_summary_unless_it_only_frames_the_batch() {
        assert_eq!(
            summary_title("\n\n**Cache warmup review**\n- Reviewed the warmup.").as_deref(),
            Some("Cache warmup review")
        );
        assert_eq!(
            summary_title("**Session metadata**\n- Batch of 4.\n\n**Queue retry fix**\n- Fixed.")
                .as_deref(),
            Some("Queue retry fix")
        );
        assert_eq!(
            summary_title("**Recommended plugins (injected)**\n- A list.\n\n## Port the parser\n")
                .as_deref(),
            Some("Port the parser")
        );
        assert_eq!(
            summary_title("- **Queue retry fix** \u{2014} in batch `b1`.\n- Fixed.").as_deref(),
            Some("Queue retry fix")
        );
        assert_eq!(
            summary_title("**Batch context**\n- Kimi Code session."),
            None
        );
        assert_eq!(summary_title("**Grok Build Session**\n- Started."), None);
    }

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
            Some("Moved the build to the new host.")
        );
        assert_eq!(
            summary_title("**Objective:** Check the cache. Then more.").as_deref(),
            Some("Objective: Check the cache.")
        );
        assert_eq!(summary_title(""), None);
        assert_eq!(summary_title("   \n"), None);
        assert_eq!(summary_title("### "), None);
        assert_eq!(summary_title("- - -"), None);
        assert_eq!(summary_title("Untitled"), None);
        assert_eq!(summary_title("1."), None);
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
