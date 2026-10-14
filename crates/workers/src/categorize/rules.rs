use kernel::memory::TagJobContext;
use kernel::memory::TagOnEntity;
use kernel::memory::TagSuggestKind;
use kernel::memory::TagSuggestionRow;
use kernel::memory::TagVocabulary;
use kernel::memory::TagVocabularyProposal;
use uuid::Uuid;
use wire::gateway::memory::TagEntityType;
use wire::gateway::memory::TagState;

use super::parse::ParsedTag;
use super::parse::TagAction;
use crate::error::WorkerError;

const MAX_SUGGESTIONS: u32 = 8;
const KEY_LIMIT: usize = 64;
const VALUE_LIMIT: usize = 128;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SuggestionPlan {
    pub(crate) rows: Vec<TagSuggestionRow>,
    pub(crate) vocabulary: Vec<TagVocabularyProposal>,
    pub(crate) proposed: u32,
    pub(crate) rejections: Vec<String>,
}

struct Fold<'a> {
    context: &'a TagJobContext,
    model: &'a str,
    allow_protected_removals: bool,
    plan: SuggestionPlan,
    placed: Vec<Placed>,
}

struct Placed {
    entity: TagEntityType,
    key: String,
    value: String,
}

struct Ready {
    action: TagAction,
    key: String,
    value: String,
    display: String,
    confidence: Option<f64>,
    reason: Option<String>,
    vocabulary: Option<TagVocabularyProposal>,
}

enum Hit<'a> {
    Fresh,
    Known(&'a TagVocabulary),
    Rejected,
}

enum Screen {
    Reject(String),
    Ready(Ready),
}

pub(crate) fn plan(
    context: &TagJobContext,
    items: &[ParsedTag],
    model: &str,
    allow_protected_removals: bool,
) -> Result<SuggestionPlan, WorkerError> {
    let mut fold = Fold {
        context,
        model,
        allow_protected_removals,
        plan: SuggestionPlan {
            rows: Vec::new(),
            vocabulary: Vec::new(),
            proposed: 0,
            rejections: Vec::new(),
        },
        placed: Vec::new(),
    };
    for item in items {
        consider(&mut fold, item)?;
    }
    Ok(fold.plan)
}

fn consider(fold: &mut Fold<'_>, item: &ParsedTag) -> Result<(), WorkerError> {
    let ready = match screen(fold, item) {
        Screen::Reject(note) => {
            fold.plan.rejections.push(note);
            return Ok(());
        }
        Screen::Ready(ready) => ready,
    };
    if fold.plan.proposed >= MAX_SUGGESTIONS {
        fold.plan
            .rejections
            .push("rejected — suggestion cap is 8".to_owned());
        return Ok(());
    }
    match ready.action {
        TagAction::Add => place_add(fold, &ready),
        TagAction::Remove => place_remove(fold, &ready),
    }
    bump(fold)
}

fn screen(fold: &Fold<'_>, item: &ParsedTag) -> Screen {
    let slug = slug(&item.value);
    if let Some(note) = shape_note(item, &slug) {
        return Screen::Reject(note);
    }
    match resolve(fold.context, &item.key, &item.value, &slug) {
        Hit::Rejected => Screen::Reject(format!(
            "rejected — taxonomy rejects \"{}:{slug}\"",
            item.key
        )),
        Hit::Known(entry) => screen_known(fold, item, entry),
        Hit::Fresh => screen_fresh(fold, item, &slug),
    }
}

fn screen_known(fold: &Fold<'_>, item: &ParsedTag, entry: &TagVocabulary) -> Screen {
    let ready = base_ready(item, &entry.value, &entry.display, None);
    match item.action {
        TagAction::Add => Screen::Ready(ready),
        TagAction::Remove => screen_remove(fold, ready),
    }
}

fn screen_fresh(fold: &Fold<'_>, item: &ParsedTag, slug: &str) -> Screen {
    let display = display_for(&item.value, slug);
    let proposal = TagVocabularyProposal {
        key: item.key.clone(),
        value: slug.to_owned(),
        display: display.clone(),
        reason: format!("proposed by {}", fold.model),
    };
    let vocabulary = match item.action {
        TagAction::Add => Some(proposal),
        TagAction::Remove => None,
    };
    let ready = base_ready(item, slug, &display, vocabulary);
    match item.action {
        TagAction::Add => Screen::Ready(ready),
        TagAction::Remove => screen_remove(fold, ready),
    }
}

fn screen_remove(fold: &Fold<'_>, ready: Ready) -> Screen {
    let Some(existing) = accepted(fold.context, &ready.key, &ready.value) else {
        return Screen::Reject(format!(
            "rejected — remove target \"{}:{}\" is not an accepted conversation tag",
            ready.key, ready.value
        ));
    };
    if protected(&existing.source) && !fold.allow_protected_removals {
        return Screen::Reject(format!(
            "rejected — protected tag \"{}:{}\"",
            ready.key, ready.value
        ));
    }
    Screen::Ready(Ready {
        display: existing.display.clone(),
        value: existing.value.clone(),
        vocabulary: None,
        ..ready
    })
}

fn base_ready(
    item: &ParsedTag,
    value: &str,
    display: &str,
    vocabulary: Option<TagVocabularyProposal>,
) -> Ready {
    Ready {
        action: item.action,
        key: item.key.clone(),
        value: value.to_owned(),
        display: display.to_owned(),
        confidence: item.confidence,
        reason: item.reason.clone(),
        vocabulary,
    }
}

fn place_add(fold: &mut Fold<'_>, ready: &Ready) {
    push_entity(fold, ready, TagEntityType::Summary);
    push_entity(fold, ready, TagEntityType::Conversation);
    if let Some(proposal) = &ready.vocabulary
        && !fold
            .plan
            .vocabulary
            .iter()
            .any(|seen| seen.key == proposal.key && seen.value == proposal.value)
    {
        fold.plan.vocabulary.push(proposal.clone());
    }
}

fn push_entity(fold: &mut Fold<'_>, ready: &Ready, entity: TagEntityType) {
    if exists(fold, entity, &ready.key, &ready.value) {
        return;
    }
    fold.plan.rows.push(TagSuggestionRow {
        kind: TagSuggestKind::Add,
        entity_type: entity,
        entity_id: entity_id(fold.context, entity),
        key: ready.key.clone(),
        value: ready.value.clone(),
        display: ready.display.clone(),
        confidence: ready.confidence,
        reason: ready.reason.clone(),
    });
    fold.placed.push(Placed {
        entity,
        key: ready.key.clone(),
        value: ready.value.clone(),
    });
}

fn place_remove(fold: &mut Fold<'_>, ready: &Ready) {
    fold.plan.rows.push(TagSuggestionRow {
        kind: TagSuggestKind::Remove,
        entity_type: TagEntityType::Conversation,
        entity_id: fold.context.conversation_id,
        key: ready.key.clone(),
        value: ready.value.clone(),
        display: ready.display.clone(),
        confidence: ready.confidence,
        reason: ready.reason.clone(),
    });
}

fn bump(fold: &mut Fold<'_>) -> Result<(), WorkerError> {
    fold.plan.proposed = fold
        .plan
        .proposed
        .checked_add(1)
        .ok_or(WorkerError::Count)?;
    Ok(())
}

fn shape_note(item: &ParsedTag, slug: &str) -> Option<String> {
    if item.key.is_empty() || slug.is_empty() {
        return Some(format!("rejected — empty value for key \"{}\"", item.key));
    }
    if item.key.contains(':') {
        return Some(format!("rejected — key \"{}\" contains a colon", item.key));
    }
    if item.key.chars().count() > KEY_LIMIT {
        return Some("rejected — key is longer than 64 characters".to_owned());
    }
    if slug.chars().count() > VALUE_LIMIT {
        return Some(format!(
            "rejected — value for key \"{}\" is longer than 128 characters",
            item.key
        ));
    }
    if confidence_ok(item.confidence) {
        None
    } else {
        Some(format!(
            "rejected — confidence for key \"{}\" is outside 0..=1",
            item.key
        ))
    }
}

fn confidence_ok(value: Option<f64>) -> bool {
    match value {
        None => true,
        Some(number) => (0.0..=1.0).contains(&number),
    }
}

fn resolve<'a>(context: &'a TagJobContext, key: &str, raw: &str, slug: &str) -> Hit<'a> {
    let mut rejected = false;
    let mut known = None;
    for entry in &context.vocabulary {
        if entry.key != key || !entry_matches(entry, raw, slug) {
            continue;
        }
        if entry.state == TagState::Rejected {
            rejected = true;
        } else if known.is_none() {
            known = Some(entry);
        }
    }
    if let Some(entry) = known {
        Hit::Known(entry)
    } else if rejected {
        Hit::Rejected
    } else {
        Hit::Fresh
    }
}

fn entry_matches(entry: &TagVocabulary, raw: &str, slug: &str) -> bool {
    entry.value == slug
        || entry
            .aliases
            .iter()
            .any(|alias| alias == raw || slug_of(alias) == slug)
}

fn accepted<'a>(context: &'a TagJobContext, key: &str, value: &str) -> Option<&'a TagOnEntity> {
    context.tags.iter().find(|tag| {
        tag.entity_type == TagEntityType::Conversation
            && tag.state == TagState::Accepted
            && tag.key == key
            && tag.value == value
    })
}

fn exists(fold: &Fold<'_>, entity: TagEntityType, key: &str, value: &str) -> bool {
    fold.context
        .tags
        .iter()
        .any(|tag| tag.entity_type == entity && tag.key == key && tag.value == value)
        || fold
            .placed
            .iter()
            .any(|tag| tag.entity == entity && tag.key == key && tag.value == value)
}

fn protected(source: &str) -> bool {
    source == "user" || source == "rule"
}

fn display_for(raw: &str, slug: &str) -> String {
    if raw == slug {
        String::new()
    } else {
        raw.to_owned()
    }
}

fn entity_id(context: &TagJobContext, entity: TagEntityType) -> Uuid {
    match entity {
        TagEntityType::Summary => context.summary_id,
        TagEntityType::Conversation => context.conversation_id,
    }
}

fn slug_of(text: &str) -> String {
    slug(text)
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::plan;
    use crate::categorize::parse::ParsedTag;
    use crate::categorize::parse::TagAction;
    use kernel::memory::TagJobContext;
    use uuid::Uuid;

    fn context() -> TagJobContext {
        TagJobContext {
            summary_id: Uuid::nil(),
            conversation_id: Uuid::nil(),
            session_key: "cli:s1".to_owned(),
            title: None,
            agent: "rivet".to_owned(),
            summary: "A deploy summary.".to_owned(),
            vocabulary: Vec::new(),
            tags: Vec::new(),
        }
    }

    #[test]
    fn a_nan_confidence_is_rejected() {
        let item = ParsedTag {
            key: "topic".to_owned(),
            value: "deploy".to_owned(),
            confidence: Some(f64::NAN),
            reason: None,
            action: TagAction::Add,
        };
        let planned = plan(&context(), &[item], "tagger-v1", false).expect("plan");
        assert_eq!(planned.proposed, 0);
        assert_eq!(planned.rows, Vec::new());
        assert_eq!(
            planned.rejections,
            vec!["rejected — confidence for key \"topic\" is outside 0..=1".to_owned()]
        );
    }
}
