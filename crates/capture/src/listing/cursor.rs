use std::path::Path;
use std::path::PathBuf;

use kernel::transcript::StoreSession;
use kernel::transcript::TranscriptReaderError;
use serde::Deserialize;
use wire::harness::HarnessId;

use super::field::Field;
use super::field::object_line;
use super::field::text_of;
use super::files::directories;
use super::files::file_times;
use super::files::head;
use super::files::newest_per_key;
use super::files::open_store;
use super::files::regular_file;
use super::text::TextElement;
use super::text::collapse;
use super::text::is_uuid;
use super::text::non_empty;
use super::time::FileTimes;
use crate::store_path::Root;

const HEAD_WINDOW: u64 = 16_384;
const TITLE_CHARS: usize = 120;

#[derive(Debug, Clone)]
struct CursorTranscript {
    id: String,
    path: PathBuf,
    times: FileTimes,
}

fn transcripts(store: &Root) -> Vec<CursorTranscript> {
    let mut found = Vec::new();
    for project in directories(store, store.canonical()) {
        if project.hidden() {
            continue;
        }
        for session in directories(store, &project.path.join("agent-transcripts")) {
            if !is_uuid(&session.name) {
                continue;
            }
            let path = session.path.join(format!("{}.jsonl", session.name));
            if !regular_file(store, &path) {
                continue;
            }
            if let Some(times) = file_times(&path) {
                found.push(CursorTranscript {
                    id: session.name,
                    path,
                    times,
                });
            }
        }
    }
    newest_per_key(
        found,
        |transcript| transcript.id.clone(),
        |transcript| transcript.times.mtime,
    )
}

fn projects(root: &Path) -> Result<Option<Root>, TranscriptReaderError> {
    open_store(&root.join("projects"))
}

pub(crate) fn list(root: &Path, limit: usize) -> Result<Vec<StoreSession>, TranscriptReaderError> {
    let Some(store) = projects(root)? else {
        return Ok(Vec::new());
    };
    let mut kept = transcripts(&store);
    kept.sort_by_key(|item| std::cmp::Reverse(item.times.mtime));
    Ok(kept.iter().take(limit).map(cursor_row).collect())
}

pub(crate) fn describe(
    root: &Path,
    id: &str,
) -> Result<Option<StoreSession>, TranscriptReaderError> {
    let Some(store) = projects(root)? else {
        return Ok(None);
    };
    Ok(transcripts(&store)
        .iter()
        .find(|transcript| transcript.id == id)
        .map(cursor_row))
}

fn cursor_row(transcript: &CursorTranscript) -> StoreSession {
    let mut row = StoreSession::new(HarnessId::Cursor, &transcript.id, transcript.times.mtime);
    row.created_at = Some(transcript.times.birth_or_mtime());
    if let Some(title) =
        head(&transcript.path, HEAD_WINDOW).and_then(|text| text.lines().find_map(user_title))
    {
        row.title = title;
    }
    row
}

#[derive(Debug, Deserialize)]
struct CursorLine {
    role: Option<Field>,
    message: Option<Field>,
}

#[derive(Debug, Deserialize)]
struct CursorMessage {
    content: Option<Field>,
}

fn user_title(line: &str) -> Option<String> {
    let parsed = object_line::<CursorLine>(line)?;
    if !parsed.role?.is_text("user") {
        return None;
    }
    let content = parsed.message?.object::<CursorMessage>()?.content?;
    let parts = match content.text() {
        Some(text) => vec![text],
        None => content
            .items()?
            .iter()
            .filter_map(Field::object::<TextElement>)
            .filter(|element| {
                element
                    .kind
                    .as_ref()
                    .is_some_and(|kind| kind.is_text("text"))
            })
            .filter_map(|element| text_of(element.text.as_ref()))
            .collect(),
    };
    let joined: Vec<String> = parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect();
    non_empty(
        collapse(joined.join("\n").trim())
            .chars()
            .take(TITLE_CHARS)
            .collect(),
    )
}
