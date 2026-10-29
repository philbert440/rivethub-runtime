use wire::JsonObject;

use super::super::field::Field;

pub(super) const RESULT_CHARS: usize = 2048;
const ARG_ENTRIES: usize = 12;
const ARG_CHARS: usize = 200;
const KEPT_INPUT_CHARS: usize = 8192;

#[derive(strum::EnumString)]
enum PromptTool {
    #[strum(serialize = "askuserquestion")]
    Joined,
    #[strum(serialize = "ask_user_question")]
    Snake,
    #[strum(serialize = "ask_user")]
    Short,
}

pub(super) fn is_prompt_tool(name: &str) -> bool {
    name.trim().to_lowercase().parse::<PromptTool>().is_ok()
}

pub(super) fn keeps_input(name: &str) -> bool {
    is_prompt_tool(name) || name.trim().eq_ignore_ascii_case("todo_write")
}

fn clip_arg(text: &str) -> String {
    if text.chars().count() <= ARG_CHARS {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(ARG_CHARS).collect();
    clipped.push('…');
    clipped
}

pub(super) fn args_summary(object: &JsonObject) -> Option<JsonObject> {
    let mut args = JsonObject::default();
    let mut kept = 0;
    for (key, value) in object.as_map() {
        if kept == ARG_ENTRIES {
            break;
        }
        let inserted = if let Some(text) = value.as_str() {
            args.insert(key, clip_arg(text))
        } else if value.is_boolean() || value.is_number() {
            args.insert(key, value)
        } else {
            continue;
        };
        if let Err(error) = inserted {
            tracing::trace!(%error, key, "tool argument is not representable and is skipped");
            continue;
        }
        kept += 1;
    }
    (kept > 0).then_some(args)
}

#[derive(Debug, serde::Serialize)]
struct Truncated {
    truncated: bool,
}

pub(super) fn kept_input(input: &Field) -> Option<JsonObject> {
    bounded(input.raw(), || input.object::<JsonObject>())
}

pub(super) fn kept_object(object: &JsonObject) -> Result<Option<JsonObject>, serde_json::Error> {
    let raw = serde_json::to_string(object)?;
    Ok(bounded(&raw, || Some(object.clone())))
}

fn bounded(raw: &str, object: impl FnOnce() -> Option<JsonObject>) -> Option<JsonObject> {
    if raw.chars().count() > KEPT_INPUT_CHARS {
        return match JsonObject::from_serialize(&Truncated { truncated: true }) {
            Ok(object) => Some(object),
            Err(error) => {
                tracing::trace!(%error, "truncated tool input marker did not serialize");
                None
            }
        };
    }
    object()
}
