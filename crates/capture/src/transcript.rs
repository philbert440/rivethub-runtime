use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::transcript::TranscriptReader;
use kernel::transcript::TranscriptReaderError;
use kernel::transcript::Turn;
use serde::Deserialize;
use wire::harness::HarnessId;
use wire::message::Role;
use wire::session_id::SessionId;

use crate::json::JsJson;

pub const HARNESS_PREFIX: &str = "claude-code:";
const TRANSCRIPT_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineKind {
    User,
    Assistant,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LineMessage {
    pub role: Role,
    pub content: LineContent,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum LineContent {
    Text(String),
    Blocks(Vec<Block>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: JsJson,
    },
    ToolResult {
        tool_use_id: Option<String>,
        content: LineContent,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub r#type: LineKind,
    pub uuid: Option<String>,
    pub timestamp: Option<String>,
    pub message: Option<LineMessage>,
    pub is_sidechain: Option<bool>,
    pub is_meta: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct ToolUse<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub input: &'a JsJson,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutcome {
    pub tool_use_id: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedLine {
    pub number: usize,
    pub reason: String,
}

#[derive(Debug, Default, Clone)]
pub struct Parsed {
    pub lines: Vec<Line>,
    pub skipped: Vec<SkippedLine>,
}

impl Line {
    #[must_use]
    pub fn timestamp_or_none(&self) -> Option<DateTime<Utc>> {
        let text = self.timestamp.as_deref()?;
        DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|at| at.with_timezone(&Utc))
    }

    #[must_use]
    pub fn text(&self) -> Option<String> {
        let message = self.message.as_ref()?;
        match &message.content {
            LineContent::Text(text) => Some(text.clone()),
            LineContent::Blocks(blocks) => Some(block_texts(blocks)),
        }
    }

    #[must_use]
    pub fn tool_result_text(&self) -> Option<String> {
        let message = self.message.as_ref()?;
        let LineContent::Blocks(blocks) = &message.content else {
            return None;
        };
        blocks.iter().find_map(|block| match block {
            Block::ToolResult { content, .. } => match content {
                LineContent::Text(text) => Some(text.clone()),
                LineContent::Blocks(nested) => {
                    let text = block_texts(nested);
                    (!text.is_empty()).then_some(text)
                }
            },
            Block::Text { .. } | Block::ToolUse { .. } | Block::Other => None,
        })
    }

    #[must_use]
    pub fn tool_uses(&self) -> Vec<ToolUse<'_>> {
        let Some(blocks) = self.message_blocks() else {
            return Vec::new();
        };
        blocks
            .iter()
            .filter_map(|block| match block {
                Block::ToolUse { id, name, input } => Some(ToolUse { id, name, input }),
                Block::Text { .. } | Block::ToolResult { .. } | Block::Other => None,
            })
            .collect()
    }

    #[must_use]
    pub fn tool_results(&self) -> Vec<ToolOutcome> {
        let Some(blocks) = self.message_blocks() else {
            return Vec::new();
        };
        blocks
            .iter()
            .filter_map(|block| match block {
                Block::ToolResult {
                    tool_use_id,
                    content,
                } => {
                    let text = match content {
                        LineContent::Text(text) => text.clone(),
                        LineContent::Blocks(nested) => block_texts(nested),
                    };
                    Some(ToolOutcome {
                        tool_use_id: tool_use_id.clone(),
                        text,
                    })
                }
                Block::Text { .. } | Block::ToolUse { .. } | Block::Other => None,
            })
            .collect()
    }

    fn message_blocks(&self) -> Option<&[Block]> {
        let message = self.message.as_ref()?;
        match &message.content {
            LineContent::Blocks(blocks) => Some(blocks),
            LineContent::Text(_) => None,
        }
    }
}

fn block_texts(blocks: &[Block]) -> String {
    let mut out = String::new();
    for block in blocks {
        if let Block::Text { text } = block {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
    }
    out
}

#[must_use]
pub fn parse_lines(text: &str) -> Parsed {
    let mut parsed = Parsed::default();
    for (offset, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Line>(line) {
            Ok(line) => parsed.lines.push(line),
            Err(error) => parsed.skipped.push(SkippedLine {
                number: offset + 1,
                reason: error.to_string(),
            }),
        }
    }
    parsed
}

pub fn report_skipped(skipped: &[SkippedLine]) {
    if skipped.is_empty() {
        return;
    }
    let numbers: Vec<usize> = skipped.iter().map(|line| line.number).collect();
    tracing::warn!(
        count = skipped.len(),
        lines = ?numbers,
        "transcript lines are not valid claude JSONL and were skipped"
    );
}

pub fn read_capped(path: &Path) -> Result<String, TranscriptReaderError> {
    let bytes = std::fs::read(path).map_err(|error| TranscriptReaderError::Read {
        message: format!("transcript {}: {error}", path.display()),
        source: Box::new(error),
    })?;
    Ok(truncate_to_lines(&bytes))
}

#[expect(
    clippy::indexing_slicing,
    reason = "the over-cap branch slices at TRANSCRIPT_MAX_BYTES, proven below the length, and the cut at a line end is at most the length"
)]
fn truncate_to_lines(bytes: &[u8]) -> String {
    if bytes.len() <= TRANSCRIPT_MAX_BYTES {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let limited = &bytes[..TRANSCRIPT_MAX_BYTES];
    let end = limited
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(limited.len(), |position| position + 1);
    String::from_utf8_lossy(&limited[..end]).into_owned()
}

#[must_use]
pub fn native_id(session: &SessionId) -> Option<&str> {
    session.as_str().strip_prefix(HARNESS_PREFIX)
}

#[derive(Debug)]
pub struct ClaudeTranscriptReader {
    store_root: PathBuf,
}

impl ClaudeTranscriptReader {
    #[must_use]
    pub fn new(store_root: PathBuf) -> Self {
        Self { store_root }
    }
}

fn turns_from_lines(lines: &[Line]) -> Vec<Turn> {
    lines
        .iter()
        .filter(|line| line.is_meta != Some(true))
        .filter_map(|line| {
            let at = line.timestamp_or_none()?;
            let role = match line.r#type {
                LineKind::User => {
                    if line.tool_result_text().is_some() {
                        Role::Tool
                    } else {
                        Role::User
                    }
                }
                LineKind::Assistant => Role::Assistant,
                LineKind::Other => return None,
            };
            let content = match role {
                Role::Tool => line.tool_result_text()?,
                Role::System | Role::User | Role::Assistant => line.text()?,
            };
            if content.is_empty() {
                return None;
            }
            Some(Turn { role, content, at })
        })
        .collect()
}

#[async_trait]
impl TranscriptReader for ClaudeTranscriptReader {
    async fn sessions(&self, store: &Path) -> Result<Vec<SessionId>, TranscriptReaderError> {
        let projects = store.join("projects");
        tokio::task::spawn_blocking(move || sessions_blocking(&projects))
            .await
            .map_err(|error| TranscriptReaderError::Unavailable(Box::new(error)))?
    }

    async fn read(&self, session: &SessionId) -> Result<Vec<Turn>, TranscriptReaderError> {
        let Some(native) = native_id(session).map(str::to_owned) else {
            return Ok(Vec::new());
        };
        let root = self.store_root.clone();
        let text = tokio::task::spawn_blocking(move || find_transcript(&root, &native))
            .await
            .map_err(|error| TranscriptReaderError::Unavailable(Box::new(error)))??;
        let parsed = parse_lines(&text);
        report_skipped(&parsed.skipped);
        Ok(turns_from_lines(&parsed.lines))
    }

    async fn tail(
        &self,
        session: &SessionId,
        since: DateTime<Utc>,
    ) -> Result<Vec<Turn>, TranscriptReaderError> {
        Ok(self
            .read(session)
            .await?
            .into_iter()
            .filter(|turn| turn.at > since)
            .collect())
    }
}

fn sessions_blocking(projects: &Path) -> Result<Vec<SessionId>, TranscriptReaderError> {
    let entries = std::fs::read_dir(projects)
        .map_err(|error| TranscriptReaderError::Unavailable(Box::new(error)))?;
    let mut sessions = Vec::new();
    for entry in entries {
        let directory =
            entry.map_err(|error| TranscriptReaderError::Unavailable(Box::new(error)))?;
        if !directory.path().is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(directory.path()) else {
            continue;
        };
        for file in files {
            let path = file
                .map_err(|error| TranscriptReaderError::Unavailable(Box::new(error)))?
                .path();
            let is_transcript = path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
                && path.is_file();
            if !is_transcript {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                sessions.push(SessionId::new(HarnessId::ClaudeCode, stem));
            }
        }
    }
    sessions.sort();
    Ok(sessions)
}

fn find_transcript(root: &Path, native: &str) -> Result<String, TranscriptReaderError> {
    let projects = root.join("projects");
    let entries = std::fs::read_dir(&projects)
        .map_err(|error| TranscriptReaderError::Unavailable(Box::new(error)))?;
    for entry in entries {
        let directory =
            entry.map_err(|error| TranscriptReaderError::Unavailable(Box::new(error)))?;
        if !directory.path().is_dir() {
            continue;
        }
        let candidate = directory.path().join(format!("{native}.jsonl"));
        if candidate.is_file() {
            return read_capped(&candidate);
        }
    }
    Ok(String::new())
}
