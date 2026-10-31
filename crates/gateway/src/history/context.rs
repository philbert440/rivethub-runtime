use std::collections::HashMap;

use kernel::transcript::{HarnessTranscriptTurn, SessionMessageRole};
use parking_lot::RwLock;
use strum::IntoEnumIterator;
use wire::events::ContextSource;
use wire::harness::HarnessId;
use wire::session_id::SessionId;

const MILLION: u64 = 1_000_000;
const EARLIER_CLAUDE_WINDOW: u64 = 200_000;
const GROK_WINDOW: u64 = 500_000;
const CODEX_WINDOW: u64 = 400_000;
const DEFAULT_WINDOW: u64 = 262_144;
const COMPACT_HEADROOM: u64 = 35_000;

#[derive(Clone, Copy, strum::EnumIter, strum::Display)]
#[strum(serialize_all = "lowercase")]
enum ClaudeFamily {
    Claude,
    Anthropic,
    Opus,
    Sonnet,
    Haiku,
    Fable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ContextStamp {
    pub(crate) window: u64,
    pub(crate) compact_at: u64,
    pub(crate) source: ContextSource,
}

#[derive(Debug, Default)]
pub(super) struct Stamps {
    sessions: RwLock<HashMap<SessionId, (usize, ContextStamp)>>,
    learned: RwLock<HashMap<u64, u64>>,
}

pub(crate) fn window_for(harness: HarnessId, model: Option<&str>) -> u64 {
    let model = model.map(str::to_lowercase).unwrap_or_default();
    if model.contains("[1m]") || model.contains("-1m") {
        MILLION
    } else if harness == HarnessId::ClaudeCode
        || ClaudeFamily::iter().any(|family| model.contains(&family.to_string()))
    {
        if long_context_claude(&model) {
            MILLION
        } else {
            EARLIER_CLAUDE_WINDOW
        }
    } else if harness == HarnessId::GrokBuild {
        GROK_WINDOW
    } else if harness == HarnessId::Codex || model.starts_with("gpt-5") || model.contains("codex") {
        CODEX_WINDOW
    } else {
        DEFAULT_WINDOW
    }
}

fn long_context_claude(model: &str) -> bool {
    let versions: Vec<u32> = model
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| (1..=2).contains(&part.len()) && part.bytes().all(|b| b.is_ascii_digit()))
        .map(|part| part.bytes().fold(0, |n, b| n * 10 + u32::from(b - b'0')))
        .collect();
    if model.contains("fable") || model.contains("mythos") {
        return true;
    }
    match versions.as_slice() {
        [] => true,
        [major, ..] if *major >= 5 => true,
        [4, minor, ..] => *minor >= 6 && !model.contains("haiku"),
        _ => false,
    }
}

fn default_compact_at(window: u64) -> u64 {
    window.saturating_sub(COMPACT_HEADROOM).max(1)
}

impl Stamps {
    pub(super) fn stamp(
        &self,
        session: &SessionId,
        harness: HarnessId,
        turns: &[HarnessTranscriptTurn],
        spawn_model: Option<&str>,
    ) -> ContextStamp {
        if let Some((count, stamp)) = self.sessions.read().get(session).copied()
            && count == turns.len()
        {
            return stamp;
        }
        let stamp = self.compute(harness, turns, spawn_model);
        self.sessions
            .write()
            .insert(session.clone(), (turns.len(), stamp));
        stamp
    }

    fn compute(
        &self,
        harness: HarnessId,
        turns: &[HarnessTranscriptTurn],
        spawn_model: Option<&str>,
    ) -> ContextStamp {
        let model = turns
            .iter()
            .rev()
            .find_map(|turn| turn.model.as_deref())
            .or(spawn_model);
        let mut window = window_for(harness, model);
        let mut source = if spawn_model.is_some() {
            ContextSource::Spawn
        } else {
            ContextSource::Default
        };
        let mut previous: Option<u64> = None;
        let prompts = turns
            .iter()
            .filter(|turn| turn.role == SessionMessageRole::Assistant)
            .filter_map(|turn| turn.usage.as_ref())
            .map(|usage| usage.prompt_tokens);
        for prompt in prompts {
            if prompt > window {
                window = prompt.div_ceil(MILLION).saturating_mul(MILLION);
                source = ContextSource::Observed;
            }
            if let Some(before) = previous
                && prompt.saturating_mul(2) < before
            {
                self.learned.write().insert(window, before);
            }
            previous = Some(prompt);
        }
        let learned = self.learned.read().get(&window).copied();
        match learned {
            Some(compact_at) => ContextStamp {
                window,
                compact_at,
                source: ContextSource::Observed,
            },
            None => ContextStamp {
                window,
                compact_at: default_compact_at(window),
                source,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use wire::harness::HarnessId;

    use super::window_for;

    #[test]
    fn current_claude_models_have_a_million_tokens_and_earlier_ones_two_hundred_thousand() {
        let window = |model: Option<&str>| window_for(HarnessId::ClaudeCode, model);
        for current in [
            None,
            Some("haiku"),
            Some("claude-haiku-5-5"),
            Some("claude-opus-5-5"),
            Some("claude-sonnet-4-6"),
            Some("claude-opus-4-6"),
            Some("claude-fable-5-1"),
            Some("claude-sonnet-4-5[1m]"),
        ] {
            assert_eq!(window(current), 1_000_000, "{current:?}");
        }
        for earlier in [
            "claude-haiku-4-5",
            "claude-sonnet-4-5-20250929",
            "claude-opus-4-1",
            "claude-sonnet-4-20250514",
            "claude-3-7-sonnet-20250219",
        ] {
            assert_eq!(window(Some(earlier)), 200_000, "{earlier}");
        }
    }
}
