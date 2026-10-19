use serde::Deserialize;
use wire::JsonObject;

use super::updates::AcpContent;
use crate::readers::message::SkippedRecord;
use crate::readers::message::unparseable;

const OUTPUT_TAG: &str = "type";

pub(super) fn tool_result(
    position: u64,
    raw: Option<JsonObject>,
    content: &[ContentItem],
) -> Result<Option<String>, SkippedRecord> {
    let Some(raw) = raw else {
        return Ok(content_text(content));
    };
    let json =
        serde_json::to_string(&raw).map_err(|error| unparseable(position, error.to_string()))?;
    if raw.get(OUTPUT_TAG).is_none() {
        return Ok(Some(json));
    }
    let known = serde_json::from_str::<KnownOutput>(&json)
        .map_err(|error| unparseable(position, error.to_string()))?;
    let rendered = known
        .render()
        .map_err(|error| unparseable(position, error.to_string()))?;
    Ok(Some(rendered.unwrap_or(json)))
}

fn content_text(content: &[ContentItem]) -> Option<String> {
    let parts: Vec<&str> = content.iter().filter_map(ContentItem::text).collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ContentItem {
    Content {
        content: AcpContent,
    },
    #[serde(other)]
    Other,
}

impl ContentItem {
    fn text(&self) -> Option<&str> {
        match self {
            Self::Content { content } => content.text(),
            Self::Other => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum KnownOutput {
    Bash(BashOutput),
    GrepSearch(GrepOutput),
    ReadFile {
        #[serde(rename = "FileContent")]
        file: Option<TextBody>,
    },
    SearchTool {
        content: Option<String>,
        result_count: Option<u64>,
    },
    #[serde(rename = "MCP")]
    Mcp(McpOutput),
    ListDir {
        #[serde(rename = "Content")]
        listing: Option<TextBody>,
    },
    Todo {
        #[serde(rename = "TodosUpdated")]
        todos: Option<TodoSummary>,
    },
    #[serde(other)]
    Other,
}

impl KnownOutput {
    fn render(self) -> Result<Option<String>, serde_json::Error> {
        Ok(match self {
            Self::Bash(output) => output.render(),
            Self::GrepSearch(output) => output.render(),
            Self::ReadFile { file } => file.map(|body| body.content),
            Self::SearchTool {
                content,
                result_count,
            } => content.map(|content| match result_count {
                Some(count) => format!("[result_count={count}]\n{content}"),
                None => content,
            }),
            Self::Mcp(output) => output.render()?,
            Self::ListDir { listing } => listing.map(|body| body.content),
            Self::Todo { todos } => todos.map(|todos| todos.summary_for_prompt),
            Self::Other => None,
        })
    }
}

#[derive(Debug, Deserialize)]
struct TextBody {
    content: String,
}

#[derive(Debug, Deserialize)]
struct TodoSummary {
    summary_for_prompt: String,
}

#[derive(Debug, Deserialize)]
struct BashOutput {
    output_for_prompt: Option<String>,
    #[serde(default)]
    output: Vec<u8>,
    exit_code: Option<i64>,
    #[serde(default)]
    timed_out: bool,
    #[serde(default)]
    truncated: bool,
}

impl BashOutput {
    fn render(self) -> Option<String> {
        let body = match self.output_for_prompt {
            Some(text) => text,
            None if self.output.is_empty() => return None,
            None => String::from_utf8_lossy(&self.output).into_owned(),
        };
        let mut status = Vec::new();
        if let Some(code) = self.exit_code {
            status.push(format!("exit_code={code}"));
        }
        if self.timed_out {
            status.push("timed_out=true".to_owned());
        }
        if self.truncated {
            status.push("truncated=true".to_owned());
        }
        if status.is_empty() {
            return Some(body);
        }
        Some(format!("{body}\n[{}]", status.join(" ")))
    }
}

#[derive(Debug, Deserialize)]
struct GrepOutput {
    output_for_prompt: Option<String>,
    stdout: Option<Vec<u8>>,
}

impl GrepOutput {
    fn render(self) -> Option<String> {
        self.output_for_prompt.or_else(|| {
            self.stdout
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        })
    }
}

#[derive(Debug, Deserialize)]
struct McpOutput {
    server_name: Option<String>,
    tool_name: Option<String>,
    output: Option<McpBody>,
}

impl McpOutput {
    fn render(self) -> Result<Option<String>, serde_json::Error> {
        let (Some(server), Some(tool), Some(output)) =
            (self.server_name, self.tool_name, self.output)
        else {
            return Ok(None);
        };
        let header = format!("[mcp {server}/{tool}]");
        Ok(Some(match output {
            McpBody::Text(text) | McpBody::Okay { okay: text } => format!("{header}\n{text}"),
            McpBody::Failure { error } => format!("{header} ERROR\n{error}"),
            McpBody::Structured(object) => format!("{header}\n{}", serde_json::to_string(&object)?),
        }))
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum McpBody {
    Text(String),
    Okay {
        #[serde(rename = "OkayOutput")]
        okay: String,
    },
    Failure {
        #[serde(rename = "ErrorOutput")]
        error: String,
    },
    Structured(JsonObject),
}
