pub const CURRENTLY: &str = "Currently";
const CURRENTLY_LIMIT: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lead {
    pub lead: String,
    pub currently: Option<String>,
}

#[must_use]
pub fn split_lead(text: &str) -> Lead {
    let text = text.trim();
    let start = text
        .match_indices(CURRENTLY)
        .map(|(at, _)| at)
        .filter(|at| sentence_start(text, *at))
        .last();
    let Some(start) = start else {
        return Lead {
            lead: text.to_owned(),
            currently: None,
        };
    };
    let lead = text.get(..start).unwrap_or_default().trim().to_owned();
    let rest = text.get(start..).unwrap_or_default();
    let end = rest
        .match_indices(". ")
        .map(|(at, _)| at.saturating_add(1))
        .next()
        .unwrap_or(rest.len());
    let sentence: String = rest
        .get(..end)
        .unwrap_or(rest)
        .trim()
        .chars()
        .take(CURRENTLY_LIMIT)
        .collect();
    Lead {
        lead,
        currently: Some(sentence).filter(|sentence| sentence.len() > CURRENTLY.len()),
    }
}

fn sentence_start(text: &str, at: usize) -> bool {
    let before = text.get(..at).unwrap_or_default().trim_end_matches(' ');
    before.is_empty() || before.ends_with(['.', '!', '?', '\n', ':'])
}

#[must_use]
pub fn stable_summary(existing: &str, proposed: &str) -> String {
    let old = split_lead(existing);
    let new = split_lead(proposed);
    let lead = if old.lead.is_empty() {
        new.lead
    } else {
        old.lead
    };
    match new.currently.or(old.currently) {
        Some(currently) if lead.is_empty() => currently,
        Some(currently) => format!("{lead} {currently}"),
        None => lead,
    }
}
