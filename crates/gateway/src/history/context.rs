use std::collections::HashMap;

use kernel::transcript::{HarnessTranscriptTurn, SessionMessageRole};
use parking_lot::RwLock;
use strum::IntoEnumIterator;
use wire::events::ContextSource;
use wire::harness::HarnessId;
use wire::session_id::SessionId;

const MILLION: u64 = 1_000_000;
const CLAUDE_WINDOW: u64 = 200_000;
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
        CLAUDE_WINDOW
    } else if harness == HarnessId::GrokBuild {
        GROK_WINDOW
    } else if harness == HarnessId::Codex || model.starts_with("gpt-5") || model.contains("codex") {
        CODEX_WINDOW
    } else {
        DEFAULT_WINDOW
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
