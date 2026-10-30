use crate::transcript::{HarnessTranscriptTurn, SessionMessageRole};
use thiserror::Error;

use crate::preview::tool_label;

const FORK_CHARS: usize = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ForkError {
    #[error("a fork keeps at least one prompt")]
    NoPrompts,
    #[error("the session has {had} prompts, so it cannot fork after prompt {asked}")]
    TooFew { asked: usize, had: usize },
}

pub fn kept(
    turns: &[HarnessTranscriptTurn],
    prompts: usize,
) -> Result<&[HarnessTranscriptTurn], ForkError> {
    if prompts == 0 {
        return Err(ForkError::NoPrompts);
    }
    let mut asked = turns
        .iter()
        .enumerate()
        .filter(|(_, turn)| turn.role == SessionMessageRole::User)
        .map(|(index, _)| index);
    if let Some(next) = asked.nth(prompts) {
        return Ok(turns.get(..next).unwrap_or(turns));
    }
    let had = turns
        .iter()
        .filter(|turn| turn.role == SessionMessageRole::User)
        .count();
    if had == prompts {
        Ok(turns)
    } else {
        Err(ForkError::TooFew {
            asked: prompts,
            had,
        })
    }
}

pub fn render(turns: &[HarnessTranscriptTurn], from: &str) -> String {
    let mut blocks: Vec<String> = turns.iter().filter_map(block).collect();
    let mut total: usize = blocks.iter().map(|block| block.chars().count()).sum();
    let mut dropped = false;
    while total > FORK_CHARS && blocks.len() > 1 {
        total -= blocks.remove(0).chars().count();
        dropped = true;
    }
    if let Some(only) = blocks.first_mut().filter(|_| total > FORK_CHARS) {
        let skip = only.chars().count() - FORK_CHARS;
        *only = format!("\u{2026}{}", only.chars().skip(skip).collect::<String>());
        dropped = true;
    }
    let mut text = format!(
        "## Earlier conversation (forked from {from})\n\nThis session is a fork of an earlier one. \
         The conversation up to the fork follows; carry on from its last reply as if you had written it."
    );
    if dropped {
        text.push_str(" Its oldest part is left out.");
    }
    for block in blocks {
        text.push_str("\n\n");
        text.push_str(&block);
    }
    text
}

fn block(turn: &HarnessTranscriptTurn) -> Option<String> {
    let tools: Vec<String> = turn
        .tools
        .iter()
        .flatten()
        .map(|tool| match &tool.args {
            Some(args) => tool_label(&tool.name, args),
            None => tool.name.clone(),
        })
        .collect();
    let said = turn.text.trim();
    if said.is_empty() && tools.is_empty() {
        return None;
    }
    let mut block = match turn.role {
        SessionMessageRole::User => "### User".to_owned(),
        SessionMessageRole::Assistant => "### Assistant".to_owned(),
    };
    if !said.is_empty() {
        block.push_str("\n\n");
        block.push_str(said);
    }
    if !tools.is_empty() {
        block.push_str("\n\nTools used: ");
        block.push_str(&tools.join("; "));
    }
    Some(block)
}

#[cfg(test)]
mod tests {
    use crate::transcript::{HarnessTranscriptTurn, SessionMessageRole};

    use super::{ForkError, kept, render};

    fn turn(role: SessionMessageRole, text: &str) -> HarnessTranscriptTurn {
        HarnessTranscriptTurn {
            role,
            text: text.to_owned(),
            thinking: None,
            tools: None,
            usage: None,
            model: None,
            stop_reason: None,
            last_block: None,
            attachments: None,
            complete: false,
            compact: false,
            started_at_ms: None,
            ended_at_ms: None,
        }
    }

    #[test]
    fn a_fork_keeps_the_prompts_it_names_and_their_replies() {
        let turns = [
            turn(SessionMessageRole::User, "first"),
            turn(SessionMessageRole::Assistant, "one"),
            turn(SessionMessageRole::Assistant, "more"),
            turn(SessionMessageRole::User, "second"),
            turn(SessionMessageRole::Assistant, "two"),
        ];
        assert_eq!(kept(&turns, 1).map(<[_]>::len), Ok(3));
        assert_eq!(kept(&turns, 2).map(<[_]>::len), Ok(5));
        assert_eq!(kept(&turns, 0), Err(ForkError::NoPrompts));
        assert_eq!(kept(&turns, 3), Err(ForkError::TooFew { asked: 3, had: 2 }));
        let text = render(&turns[..3], "\"Plan\"");
        assert!(text.starts_with("## Earlier conversation (forked from \"Plan\")"));
        assert!(
            text.ends_with("### User\n\nfirst\n\n### Assistant\n\none\n\n### Assistant\n\nmore")
        );
    }
}
