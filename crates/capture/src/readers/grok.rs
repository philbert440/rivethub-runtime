use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::transcript::TranscriptReader;
use kernel::transcript::TranscriptReaderError;
use kernel::transcript::Turn;
use serde::Deserialize;
use wire::JsonObject;
use wire::harness::HarnessId;
use wire::message::Role;
use wire::session_id::SessionId;

use super::message::Mapped;
use super::message::SkipRule;
use super::message::SkippedRecord;
use super::message::StoreRole;
use super::message::TextTurn;
use super::message::ToolFields;
use super::message::TranscriptSlice;
use super::message::assemble;
use super::message::blocking;
use super::message::native;
use super::message::parse_json;
use super::message::project_turns;
use super::message::push_mapped;
use super::message::report;
use super::message::text_turn;
use super::message::tool_turn;
use super::message::unknown;
use super::scan::lines;
use super::scan::read_optional;
use crate::store_path::Child;
use crate::store_path::DirState;
use crate::store_path::Root;
use crate::store_path::dir_state;
use crate::store_path::session_component;

const FORMAT: &str = "grok-chat-history";
const PREFIX: &str = "grok-build:";

#[derive(Debug)]
pub struct GrokTranscriptReader {
    root: PathBuf,
}

impl GrokTranscriptReader {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn messages(
        &self,
        session: &SessionId,
        offset: u64,
    ) -> Result<TranscriptSlice, TranscriptReaderError> {
        let Some(native_id) = native(session, PREFIX).map(str::to_owned) else {
            return Ok(TranscriptSlice::empty(offset));
        };
        let root = self.root.clone();
        let session = session.clone();
        let slice = blocking(move || load(&root, &session, &native_id, offset)).await?;
        report(FORMAT, &slice.skipped);
        Ok(slice)
    }
}

#[async_trait]
impl TranscriptReader for GrokTranscriptReader {
    async fn sessions(&self, store: &Path) -> Result<Vec<SessionId>, TranscriptReaderError> {
        let store = store.to_path_buf();
        blocking(move || list_sessions(&store)).await
    }

    async fn read(&self, session: &SessionId) -> Result<Vec<Turn>, TranscriptReaderError> {
        project_turns(&self.messages(session, 0).await?)
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

fn list_sessions(store: &Path) -> Result<Vec<SessionId>, TranscriptReaderError> {
    let mut sessions = Vec::new();
    for (id, _) in session_dirs(&Root::open(store)?)? {
        sessions.push(SessionId::new(HarnessId::GrokBuild, &id));
    }
    sessions.sort();
    sessions.dedup();
    Ok(sessions)
}

fn load(
    store: &Path,
    session: &SessionId,
    native_id: &str,
    offset: u64,
) -> Result<TranscriptSlice, TranscriptReaderError> {
    session_component(native_id)?;
    let root = Root::open(store)?;
    let Some(dir) = session_dirs(&root)?
        .into_iter()
        .find_map(|(id, path)| (id == native_id).then_some(path))
    else {
        return Ok(TranscriptSlice::empty(offset));
    };
    let history = dir.join("chat_history.jsonl");
    let Some(text) = read_optional(&root, &history)? else {
        return Ok(TranscriptSlice::empty(offset));
    };
    let mut slice = map_history(session, &text, offset)?;
    if offset == 0 {
        slice.skipped.extend(metadata(&root, &dir, native_id)?);
    }
    Ok(slice)
}

fn session_dirs(root: &Root) -> Result<Vec<(String, PathBuf)>, TranscriptReaderError> {
    let sessions = root.canonical().join("sessions");
    if dir_state(root, &sessions)? == DirState::Absent {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for encoded in root.directories(&sessions)? {
        for session_dir in root.directories(&encoded)? {
            let name = utf8_stem_dir(&session_dir)?;
            if uuid::Uuid::parse_str(&name).is_err() {
                continue;
            }
            if root.classify(&session_dir.join("chat_history.jsonl"))? == Child::File {
                found.push((name, session_dir));
            }
        }
    }
    Ok(found)
}

fn utf8_stem_dir(path: &Path) -> Result<String, TranscriptReaderError> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| crate::store_path::refused(path, "directory name is not utf-8"))
}

fn map_history(
    session: &SessionId,
    text: &str,
    offset: u64,
) -> Result<TranscriptSlice, TranscriptReaderError> {
    let (records, last) = lines(text)?;
    let mut drafts = Vec::new();
    let mut skipped = Vec::new();
    for record in &records {
        let mapped = match parse_json::<GrokHistory>(record.number, &record.text) {
            Ok(line) => map_history_line(session, record.number, line)?,
            Err(skipped) => Mapped::Skip(skipped),
        };
        push_mapped(mapped, &mut drafts, &mut skipped);
    }
    Ok(assemble(offset, last, drafts, skipped))
}

fn map_history_line(
    session: &SessionId,
    position: u64,
    line: GrokHistory,
) -> Result<Mapped, TranscriptReaderError> {
    match line {
        GrokHistory::Message {
            role,
            content,
            timestamp,
        } => {
            let Some(role) = role.known() else {
                return Ok(speaker_skip(position));
            };
            text_turn(&TextTurn {
                session,
                position,
                role,
                content: &content,
                at: &timestamp,
                format: FORMAT,
            })
        }
        GrokHistory::Tool {
            name,
            arguments,
            result,
            timestamp,
        } => {
            let content = result.clone();
            tool_turn(
                &TextTurn {
                    session,
                    position,
                    role: Role::Tool,
                    content: &content,
                    at: &timestamp,
                    format: FORMAT,
                },
                ToolFields {
                    name: Some(name),
                    arguments: Some(arguments),
                    result: Some(result),
                },
            )
        }
        GrokHistory::Unknown => Ok(Mapped::Skip(unknown(position, "unknown grok record"))),
    }
}

fn metadata(
    root: &Root,
    dir: &Path,
    native_id: &str,
) -> Result<Vec<SkippedRecord>, TranscriptReaderError> {
    let mut skipped = Vec::new();
    if let Some(text) = read_optional(root, &dir.join("summary.json"))? {
        skipped.extend(summary_skips(native_id, &text));
    }
    if let Some(text) = read_optional(root, &dir.join("updates.jsonl"))? {
        skipped.extend(update_skips(&text)?);
    }
    Ok(skipped)
}

fn summary_skips(native_id: &str, text: &str) -> Vec<SkippedRecord> {
    match serde_json::from_str::<GrokSummary>(text) {
        Ok(summary) if summary.session_id == native_id => Vec::new(),
        Ok(summary) => vec![super::message::unparseable(
            0,
            format!(
                "summary session_id {} does not match directory {native_id}",
                summary.session_id
            ),
        )],
        Err(error) => vec![super::message::unparseable(0, error.to_string())],
    }
}

fn update_skips(text: &str) -> Result<Vec<SkippedRecord>, TranscriptReaderError> {
    let (records, _) = lines(text)?;
    let mut skipped = Vec::new();
    for record in records {
        match parse_json::<GrokUpdate>(record.number, &record.text) {
            Ok(GrokUpdate::Parent { parent_session_id }) => {
                if parent_session_id.is_empty() {
                    skipped.push(super::message::unparseable(
                        record.number,
                        "parent link is empty",
                    ));
                }
            }
            Ok(GrokUpdate::Unknown) => skipped.push(unknown(record.number, "unknown grok update")),
            Err(record) => skipped.push(record),
        }
    }
    Ok(skipped)
}

fn speaker_skip(position: u64) -> Mapped {
    Mapped::Skip(SkippedRecord {
        position,
        rule: SkipRule::UnrecognizedSpeaker,
        detail: "grok message role".to_owned(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum GrokHistory {
    Message {
        role: StoreRole,
        content: String,
        timestamp: String,
    },
    Tool {
        name: String,
        arguments: JsonObject,
        result: String,
        timestamp: String,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct GrokSummary {
    session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum GrokUpdate {
    Parent {
        parent_session_id: String,
    },
    #[serde(other)]
    Unknown,
}
