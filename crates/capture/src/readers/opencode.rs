use std::path::Path;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::transcript::TranscriptReader;
use kernel::transcript::TranscriptReaderError;
use kernel::transcript::Turn;
use wire::harness::HarnessId;
use wire::message::Role;
use wire::session_id::SessionId;

use super::harness_db::DbRows;
use super::harness_db::HarnessDb;
use super::harness_db::OpencodePart;
use super::harness_db::PartKind;
use super::harness_db::Speaker;
use super::harness_db::read_error;

use super::message::Mapped;
use super::message::SkipRule;
use super::message::SkippedRecord;
use super::message::TextTurn;
use super::message::ToolFields;
use super::message::TranscriptSlice;
use super::message::arguments;
use super::message::assemble;
use super::message::blocking;
use super::message::native;
use super::message::project_turns;
use super::message::push_mapped;
use super::message::report;
use super::message::row_offset;
use super::message::row_position;
use super::message::text_turn;
use super::message::tool_turn;
use super::message::unknown;
use super::sqlite::database_file;
use super::sqlite::require_component;

const FORMAT: &str = "opencode-part";
const PREFIX: &str = "opencode:";
const FILE: &str = "opencode.db";

#[derive(Debug)]
pub struct OpencodeTranscriptReader {
    root: PathBuf,
}

impl OpencodeTranscriptReader {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub async fn messages(
        &self,
        session: &SessionId,
        offset: u64,
    ) -> Result<TranscriptSlice, TranscriptReaderError> {
        let Some(native_id) = native(session, PREFIX) else {
            return Ok(TranscriptSlice::empty(offset));
        };
        let root = self.root.clone();
        let native_owned = native_id.to_owned();
        let Some(path) = blocking(move || located(&root, &native_owned)).await? else {
            return Ok(TranscriptSlice::empty(offset));
        };
        let after = row_offset(offset)?;
        let rows = HarnessDb::opencode_parts(&path, native_id, after)
            .await
            .map_err(read_error)?;
        let slice = match rows {
            DbRows::Unreadable => TranscriptSlice::empty(offset),
            DbRows::Ready(rows) => map_parts(session, &rows, offset)?,
        };
        report(FORMAT, &slice.skipped);
        Ok(slice)
    }
}

#[async_trait]
impl TranscriptReader for OpencodeTranscriptReader {
    async fn sessions(&self, store: &Path) -> Result<Vec<SessionId>, TranscriptReaderError> {
        let store = store.to_path_buf();
        let Some(path) = blocking(move || database_file(&store, FILE)).await? else {
            return Ok(Vec::new());
        };
        match HarnessDb::opencode_sessions(&path)
            .await
            .map_err(read_error)?
        {
            DbRows::Unreadable => Ok(Vec::new()),
            DbRows::Ready(ids) => {
                let mut sessions = ids
                    .iter()
                    .map(|id| SessionId::new(HarnessId::Opencode, id))
                    .collect::<Vec<_>>();
                sessions.sort();
                sessions.dedup();
                Ok(sessions)
            }
        }
    }

    async fn read(&self, session: &SessionId) -> Result<Vec<Turn>, TranscriptReaderError> {
        Ok(project_turns(&self.messages(session, 0).await?))
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

fn located(store: &Path, native_id: &str) -> Result<Option<PathBuf>, TranscriptReaderError> {
    require_component(native_id)?;
    database_file(store, FILE)
}

fn map_parts(
    session: &SessionId,
    rows: &[OpencodePart],
    offset: u64,
) -> Result<TranscriptSlice, TranscriptReaderError> {
    let mut drafts = Vec::new();
    let mut skipped = Vec::new();
    let mut last = offset;
    for row in rows {
        let position = row_position(row.row_id)?;
        last = last.max(position);
        push_mapped(map_part(session, position, row)?, &mut drafts, &mut skipped);
    }
    Ok(assemble(offset, last, drafts, skipped))
}

fn map_part(
    session: &SessionId,
    position: u64,
    row: &OpencodePart,
) -> Result<Mapped, TranscriptReaderError> {
    if row.kind == PartKind::Unrecognized {
        return Ok(Mapped::Skip(unknown(position, "unknown opencode part")));
    }
    let Speaker::Known(role) = &row.speaker else {
        return Ok(Mapped::Skip(unrecognized(position, &row.speaker)));
    };
    if row.kind == PartKind::Text {
        let content = row.text.as_deref().unwrap_or_default();
        return text_turn(&TextTurn {
            session,
            position,
            role: *role,
            content,
            at: &row.at,
            format: FORMAT,
        });
    }
    let Some(content) = tool_content(row) else {
        return Ok(Mapped::None);
    };
    let arguments = match row.tool_arguments.as_deref() {
        Some(text) => match arguments(position, text) {
            Ok(arguments) => Some(arguments),
            Err(skipped) => return Ok(Mapped::Skip(skipped)),
        },
        None => None,
    };
    tool_turn(
        &TextTurn {
            session,
            position,
            role: Role::Tool,
            content: &content,
            at: &row.at,
            format: FORMAT,
        },
        ToolFields {
            name: row.tool_name.clone(),
            arguments,
            result: row.tool_result.clone(),
        },
    )
}

fn tool_content(row: &OpencodePart) -> Option<String> {
    if let Some(text) = &row.text
        && !text.is_empty()
    {
        return Some(text.clone());
    }
    row.tool_result
        .as_ref()
        .filter(|text| !text.is_empty())
        .cloned()
}

fn unrecognized(position: u64, speaker: &Speaker) -> SkippedRecord {
    let Speaker::Unrecognized { text, parse_error } = speaker else {
        return SkippedRecord {
            position,
            rule: SkipRule::UnrecognizedSpeaker,
            detail: "opencode speaker".to_owned(),
        };
    };
    SkippedRecord {
        position,
        rule: SkipRule::UnrecognizedSpeaker,
        detail: format!("{text}: {parse_error}"),
    }
}
