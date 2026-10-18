use std::fs::File;
use std::io::Read;
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
use super::files::open_store;
use super::files::regular_file;
use super::files::unreadable;
use super::files::whole_file;
use super::text::clip;
use super::text::collapse;
use super::text::non_empty;
use super::text::segment_safe;
use super::text::user_text;
use super::time::RawTime;
use super::time::finite_or_date;
use crate::store_path::Root;

const DIR_PREFIX: &str = "session_";
const TITLE_CHARS: usize = 120;
const WIRE_CHUNK: usize = 64 * 1024;
const WIRE_BUDGET: usize = 1024 * 1024;

#[derive(Debug, Clone)]
struct KimiDir {
    id: String,
    dir: PathBuf,
    mtime: i64,
}

fn session_dirs(store: &Root, only: Option<&str>) -> Vec<KimiDir> {
    let mut found = Vec::new();
    for bucket in directories(store, store.canonical()) {
        for entry in entries(store, &bucket.path) {
            if !entry.name.starts_with(DIR_PREFIX) || only.is_some_and(|id| id != entry.name) {
                continue;
            }
            let state = entry.path.join("state.json");
            if !regular_file(store, &state) {
                continue;
            }
            if let Some(times) = file_times(&state) {
                found.push(KimiDir {
                    id: entry.name,
                    dir: entry.path,
                    mtime: times.mtime,
                });
            }
        }
    }
    found.sort_by_key(|item| std::cmp::Reverse(item.mtime));
    found
}

fn sessions(root: &Path) -> Result<Option<Root>, TranscriptReaderError> {
    open_store(&root.join("sessions"))
}

pub(crate) fn list(root: &Path, limit: usize) -> Result<Vec<StoreSession>, TranscriptReaderError> {
    let Some(store) = sessions(root)? else {
        return Ok(Vec::new());
    };
    Ok(session_dirs(&store, None)
        .iter()
        .take(limit)
        .filter_map(kimi_row)
        .collect())
}

pub(crate) fn describe(
    root: &Path,
    id: &str,
) -> Result<Option<StoreSession>, TranscriptReaderError> {
    if !segment_safe(id) {
        return Ok(None);
    }
    let Some(store) = sessions(root)? else {
        return Ok(None);
    };
    Ok(session_dirs(&store, Some(id)).first().and_then(kimi_row))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiState {
    title: Option<Field>,
    last_prompt: Option<Field>,
    updated_at: Option<Field>,
    created_at: Option<Field>,
}

fn kimi_row(found: &KimiDir) -> Option<StoreSession> {
    let state = whole_file(&found.dir.join("state.json"))
        .and_then(|text| object_line::<KimiState>(&text))?;
    let updated_at = finite_or_date(&RawTime::from_field(state.updated_at.as_ref()))
        .filter(|ms| *ms != 0)
        .unwrap_or(found.mtime);
    let mut row = StoreSession::new(HarnessId::KimiCode, &found.id, updated_at);
    let typed = [state.title.as_ref(), state.last_prompt.as_ref()]
        .into_iter()
        .find_map(|field| text_of(field).and_then(|text| non_empty(text.trim().to_owned())))
        .or_else(|| kimi_wire_title(&found.dir));
    if let Some(title) =
        typed.and_then(|title| non_empty(collapse(&title).chars().take(TITLE_CHARS).collect()))
    {
        row.title = title;
    }
    row.created_at =
        finite_or_date(&RawTime::from_field(state.created_at.as_ref())).filter(|ms| *ms != 0);
    Some(row)
}

#[derive(Debug, Deserialize)]
struct WireLine {
    #[serde(rename = "type")]
    kind: Option<Field>,
    message: Option<Field>,
}

#[derive(Debug, Deserialize)]
struct WireMessage {
    origin: Option<Field>,
    content: Option<Field>,
}

#[derive(Debug, Deserialize)]
struct Origin {
    kind: Option<Field>,
}

fn wire_text(line: &str) -> Option<String> {
    let parsed = object_line::<WireLine>(line)?;
    if !parsed.kind.as_ref()?.is_text("context.append_message") {
        return None;
    }
    let message = parsed.message?.object::<WireMessage>()?;
    if !message.origin?.object::<Origin>()?.kind?.is_text("user") {
        return None;
    }
    non_empty(clip(&user_text(message.content.as_ref()?)?, TITLE_CHARS))
}

pub(crate) fn kimi_wire_title(dir: &Path) -> Option<String> {
    let path = dir.join("agents").join("main").join("wire.jsonl");
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            unreadable(&path, &error);
            return None;
        }
    };
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = vec![0_u8; WIRE_CHUNK];
    let mut total = 0;
    loop {
        let read = match file.read(&mut chunk) {
            Ok(read) => read,
            Err(error) => {
                unreadable(&path, &error);
                return None;
            }
        };
        if read == 0 {
            return wire_text(&String::from_utf8_lossy(&pending));
        }
        pending.extend(chunk.iter().take(read));
        total += read;
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=newline).collect();
            if let Some(title) = wire_text(&String::from_utf8_lossy(&line)) {
                return Some(title);
            }
        }
        if total >= WIRE_BUDGET {
            return None;
        }
    }
}
