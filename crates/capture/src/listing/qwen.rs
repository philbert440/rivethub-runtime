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
use super::files::entries;
use super::files::file_times;
use super::files::first_cwd;
use super::files::newest_per_key;
use super::files::open_store;
use super::files::tail_window;
use super::text::clip;
use super::text::is_uuid;
use super::text::non_empty;
use super::time::FileTimes;
use crate::store_path::Root;

const TITLE_CHARS: usize = 120;

#[derive(Debug, Clone)]
struct Chat {
    id: String,
    path: PathBuf,
    times: FileTimes,
}

fn chats(store: &Root) -> Vec<Chat> {
    let mut found = Vec::new();
    for bucket in directories(store, store.canonical()) {
        if bucket.hidden() {
            continue;
        }
        for entry in entries(store, &bucket.path.join("chats")) {
            let Some(id) = entry
                .name
                .strip_suffix(".jsonl")
                .filter(|stem| is_uuid(stem))
            else {
                continue;
            };
            if entry.is_file()
                && let Some(times) = file_times(&entry.path)
            {
                found.push(Chat {
                    id: id.to_owned(),
                    path: entry.path.clone(),
                    times,
                });
            }
        }
    }
    newest_per_key(found, |chat| chat.id.clone(), |chat| chat.times.mtime)
}

fn projects(root: &Path) -> Result<Option<Root>, TranscriptReaderError> {
    open_store(&root.join("projects"))
}

fn newest(root: &Path, id: &str) -> Result<Option<Chat>, TranscriptReaderError> {
    let Some(store) = projects(root)? else {
        return Ok(None);
    };
    Ok(chats(&store).into_iter().find(|chat| chat.id == id))
}

pub(crate) fn list(root: &Path, limit: usize) -> Result<Vec<StoreSession>, TranscriptReaderError> {
    let Some(store) = projects(root)? else {
        return Ok(Vec::new());
    };
    let mut kept = chats(&store);
    kept.sort_by_key(|item| std::cmp::Reverse(item.times.mtime));
    Ok(kept.iter().take(limit).map(chat_row).collect())
}

pub(crate) fn describe(
    root: &Path,
    id: &str,
) -> Result<Option<StoreSession>, TranscriptReaderError> {
    Ok(newest(root, id)?.as_ref().map(chat_row))
}

pub(crate) fn store_cwd(root: &Path, id: &str) -> Result<Option<String>, TranscriptReaderError> {
    match newest(root, id)? {
        Some(chat) => first_cwd(&chat.path),
        None => Ok(None),
    }
}

fn chat_row(chat: &Chat) -> StoreSession {
    let mut row = StoreSession::new(HarnessId::QwenCode, &chat.id, chat.times.mtime);
    row.created_at = Some(chat.times.birth);
    row.cwd = match first_cwd(&chat.path) {
        Ok(cwd) => cwd,
        Err(error) => {
            tracing::debug!(%error, "qwen chat cwd is unreadable");
            None
        }
    };
    if let Some(title) = tail_window(&chat.path).and_then(|text| text.lines().find_map(user_title))
    {
        row.title = title;
    }
    row
}

#[derive(Debug, Deserialize)]
struct QwenLine {
    #[serde(rename = "type")]
    kind: Option<Field>,
    provenance: Option<Field>,
    message: Option<Field>,
}

#[derive(Debug, Deserialize)]
struct QwenMessage {
    parts: Option<Field>,
}

#[derive(Debug, Deserialize)]
struct QwenPart {
    text: Option<Field>,
}

fn user_title(line: &str) -> Option<String> {
    let parsed = object_line::<QwenLine>(line)?;
    if !parsed.kind?.is_text("user") || !parsed.provenance?.is_text("real_user") {
        return None;
    }
    let message = parsed.message?.object::<QwenMessage>()?;
    let joined = message
        .parts
        .and_then(|parts| parts.items())
        .unwrap_or_default()
        .iter()
        .filter_map(Field::object::<QwenPart>)
        .filter_map(|part| text_of(part.text.as_ref()))
        .collect::<Vec<_>>()
        .join("\n");
    non_empty(clip(&joined, TITLE_CHARS))
}
