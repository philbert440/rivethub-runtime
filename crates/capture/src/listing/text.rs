use super::field::Field;
use super::field::text_of;

const QUERY_OPEN: &str = "<user_query>";
const QUERY_CLOSE: &str = "</user_query>";
const PASTE_OPEN: &str = "<pasted_content id=\"";
const PASTE_CLOSE: &str = "</pasted_content";

pub(crate) fn segment_safe(value: &str) -> bool {
    !value.is_empty() && !value.contains('/') && !value.contains("..") && value != "."
}

pub(crate) fn is_uuid(value: &str) -> bool {
    value.len() == 36 && uuid::Uuid::try_parse(value).is_ok()
}

pub(crate) fn clip(value: &str, limit: usize) -> String {
    value.trim().chars().take(limit).collect()
}

pub(crate) fn clip_field(field: Option<&Field>, limit: usize) -> String {
    text_of(field).map_or_else(String::new, |text| clip(&text, limit))
}

pub(crate) fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

pub(crate) fn collapse(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn user_text(content: &Field) -> Option<String> {
    let text = match content.text() {
        Some(text) => text,
        None => text_elements(&content.items()?).join("\n"),
    };
    human_turn(&text)
}

pub(crate) fn text_elements(items: &[Field]) -> Vec<String> {
    items
        .iter()
        .filter_map(Field::object::<TextElement>)
        .filter(|element| {
            element
                .kind
                .as_ref()
                .is_some_and(|kind| kind.is_text("text"))
        })
        .filter_map(|element| text_of(element.text.as_ref()))
        .collect()
}

fn not_human(text: &str) -> bool {
    [
        "<command-",
        "<local-command",
        "<system-reminder",
        "<task-notification",
        "<user_info",
        "<environment_context>",
        "<skills_instructions>",
        "<multi_agent_",
        "Caveat:",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix))
}

pub(crate) fn human_turn(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || not_human(trimmed) {
        return None;
    }
    let Some(query) = trimmed.strip_prefix(QUERY_OPEN) else {
        return Some(trimmed.to_owned());
    };
    let inner = query
        .split_once(QUERY_CLOSE)
        .map_or(query, |(inner, _)| inner);
    non_empty(inner.trim().to_owned())
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct TextElement {
    #[serde(rename = "type")]
    pub(crate) kind: Option<Field>,
    pub(crate) text: Option<Field>,
}

pub(crate) fn strip_paste(text: &str) -> String {
    if !text.contains("pasted_content") {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut rest = text;
    let mut replaced = false;
    while let Some(start) = rest.find(PASTE_OPEN) {
        let (before, from_open) = rest.split_at(start);
        out.push_str(before);
        if let Some(block) = paste_block(from_open) {
            out.push_str(block.body);
            rest = block.rest;
            replaced = true;
        } else {
            let (literal, after) = from_open.split_at(PASTE_OPEN.len().min(from_open.len()));
            out.push_str(literal);
            rest = after;
        }
    }
    if !replaced {
        return text.to_owned();
    }
    out.push_str(rest);
    out.trim().to_owned()
}

struct PasteBlock<'a> {
    body: &'a str,
    rest: &'a str,
}

fn paste_block(from_open: &str) -> Option<PasteBlock<'_>> {
    let after_open = from_open.strip_prefix(PASTE_OPEN)?;
    let (id, after_id) = after_open.split_once('"')?;
    if id.is_empty() {
        return None;
    }
    let after_tag = after_id.strip_prefix('>')?;
    let body_start = after_tag.strip_prefix('\n').unwrap_or(after_tag);
    let tagged = format!("{PASTE_CLOSE} id=\"{id}\">");
    let bare = format!("{PASTE_CLOSE}>");
    let mut searched = 0;
    while let Some(found) = body_start
        .get(searched..)
        .and_then(|tail| tail.find(PASTE_CLOSE))
    {
        let at = searched + found;
        let (before_close, from_close) = body_start.split_at(at);
        let closer = [tagged.as_str(), bare.as_str()]
            .into_iter()
            .find(|closer| from_close.starts_with(closer));
        if let Some(closer) = closer {
            let body = before_close.strip_suffix('\n').unwrap_or(before_close);
            let rest = from_close.get(closer.len()..)?;
            return Some(PasteBlock { body, rest });
        }
        searched = at + PASTE_CLOSE.len();
    }
    None
}
