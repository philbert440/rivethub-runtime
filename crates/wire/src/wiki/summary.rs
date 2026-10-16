use serde::{Deserialize, Serialize};

pub const SUMMARY_LIMIT: usize = 2400;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CappedSummary {
    pub kept: String,
    pub overflow: String,
}

#[must_use]
pub fn merge_summary_text(existing: &str, delta: &str) -> String {
    let existing = existing.trim();
    let delta = delta.trim();
    if delta.is_empty() || existing.contains(delta) {
        existing.to_owned()
    } else if existing.is_empty() || delta.starts_with(existing) {
        delta.to_owned()
    } else {
        format!("{existing}\n\n{delta}")
    }
}

#[must_use]
pub fn cap_summary(text: &str, limit: usize) -> CappedSummary {
    let Some((hard, _)) = text.char_indices().nth(limit) else {
        return CappedSummary {
            kept: text.to_owned(),
            overflow: String::new(),
        };
    };
    let cut = text
        .match_indices("\n\n")
        .map(|(index, _)| index)
        .take_while(|index| *index <= hard)
        .last()
        .filter(|paragraph| 2 * text[..*paragraph].chars().count() >= limit)
        .unwrap_or(hard);
    let (kept, overflow) = text.split_at(cut);
    CappedSummary {
        kept: kept.trim_end().to_owned(),
        overflow: overflow.trim_start().to_owned(),
    }
}
