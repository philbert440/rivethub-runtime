use serde::Deserialize;

use crate::error::WorkerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TagAction {
    Add,
    Remove,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ParsedTag {
    pub(crate) key: String,
    pub(crate) value: String,
    pub(crate) confidence: Option<f64>,
    pub(crate) reason: Option<String>,
    pub(crate) action: TagAction,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelTag {
    key: String,
    value: String,
    #[serde(default)]
    confidence: Option<f64>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    action: Option<ModelAction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ModelAction {
    Add,
    Remove,
}

pub(crate) fn parse_suggestions(content: &str) -> Result<Vec<ParsedTag>, WorkerError> {
    if content.is_empty() {
        return Err(malformed("the model returned no tag suggestions"));
    }
    let items: Vec<ModelTag> = serde_json::from_str(content)
        .map_err(|error| malformed(format!("tag suggestions are not a JSON array: {error}")))?;
    Ok(items.into_iter().map(parsed).collect())
}

fn parsed(item: ModelTag) -> ParsedTag {
    ParsedTag {
        key: item.key,
        value: item.value,
        confidence: item.confidence,
        reason: item.reason,
        action: match item.action {
            Some(ModelAction::Remove) => TagAction::Remove,
            Some(ModelAction::Add) | None => TagAction::Add,
        },
    }
}

fn malformed(message: impl Into<String>) -> WorkerError {
    WorkerError::TagSuggestions {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_suggestions;

    #[test]
    fn an_absent_reason_stays_absent() {
        let omitted = parse_suggestions(r#"[{"key":"topic","value":"notes"}]"#).expect("omitted");
        assert_eq!(omitted.first().expect("row").reason.as_deref(), None);
        let null =
            parse_suggestions(r#"[{"key":"topic","value":"notes","reason":null}]"#).expect("null");
        assert_eq!(null.first().expect("row").reason.as_deref(), None);
        let text = parse_suggestions(r#"[{"key":"topic","value":"notes","reason":"seen"}]"#)
            .expect("text");
        assert_eq!(text.first().expect("row").reason.as_deref(), Some("seen"));
        let empty =
            parse_suggestions(r#"[{"key":"topic","value":"notes","reason":""}]"#).expect("empty");
        assert_eq!(empty.first().expect("row").reason.as_deref(), Some(""));
    }
}
