use std::path::Path;

use crate::baseline::Baseline;
use crate::error::Result;

pub fn check(root: &Path, baseline: &Baseline, findings: &mut Vec<String>) -> Result<()> {
    crate::rustsrc::check(root, baseline, "REASON", findings)
}

pub(crate) fn imitation_word(reason: &str) -> Option<&'static str> {
    let lower = reason.to_ascii_lowercase();
    for (needle, word) in [
        ("mirror", "mirror"),
        ("recorded", "recorded"),
        ("typescript", "TypeScript"),
        ("javascript", "JavaScript"),
    ] {
        if lower.contains(needle) {
            return Some(word);
        }
    }
    if let Some(position) = lower.find("js") {
        let before = lower[..position].chars().next_back();
        let after = lower[position + 2..].chars().next();
        let boundary = |ch: Option<char>| ch.is_none_or(|ch| !ch.is_ascii_alphanumeric());
        if boundary(before) && boundary(after) {
            return Some("JS");
        }
    }
    None
}
