use std::path::Path;
use std::path::PathBuf;

use kernel::transcript::StoreSession;
use kernel::transcript::TranscriptReaderError;
use serde::Deserialize;
use wire::harness::HarnessId;

use super::field::Field;
use super::field::object_line;
use super::field::text_of;
use super::files::Entry;
use super::files::entries;
use super::files::file_times;
use super::files::newest_per_key;
use super::files::open_store;
use super::files::tail_window;
use super::text::TextElement;
use super::text::clip;
use super::text::human_turn;
use super::text::non_empty;
use super::time::FileTimes;
use crate::readers::pi::pi_id;
use crate::store_path::Root;

const TITLE_CHARS: usize = 120;

#[derive(Debug, Clone)]
struct PiFile {
    prefix: String,
    id: String,
    path: PathBuf,
    times: FileTimes,
}

fn candidate(entry: &Entry) -> Option<PiFile> {
    if !entry.is_file() {
        return None;
    }
    let (prefix, id) = pi_id(entry.name.strip_suffix(".jsonl")?)?;
    Some(PiFile {
        prefix,
        id,
        path: entry.path.clone(),
        times: file_times(&entry.path)?,
    })
}

fn files(store: &Root) -> Vec<PiFile> {
    let mut found = Vec::new();
    for entry in entries(store, store.canonical()) {
        if entry.hidden() {
            continue;
        }
        if entry.is_dir() {
            found.extend(
                entries(store, &entry.path)
                    .iter()
                    .filter(|inner| !inner.hidden())
                    .filter_map(candidate),
            );
        } else if let Some(file) = candidate(&entry) {
            found.push(file);
        }
    }
    found
}

fn sessions(root: &Path) -> Result<Option<Root>, TranscriptReaderError> {
    open_store(&root.join("agent").join("sessions"))
}

pub(crate) fn list(root: &Path, limit: usize) -> Result<Vec<StoreSession>, TranscriptReaderError> {
    let Some(store) = sessions(root)? else {
        return Ok(Vec::new());
    };
    let mut kept = newest_per_key(
        files(&store),
        |file| file.id.clone(),
        |file| file.times.mtime,
    );
    kept.sort_by_key(|item| std::cmp::Reverse(item.times.mtime));
    Ok(kept.iter().take(limit).map(pi_row).collect())
}

pub(crate) fn describe(
    root: &Path,
    id: &str,
) -> Result<Option<StoreSession>, TranscriptReaderError> {
    let Some(store) = sessions(root)? else {
        return Ok(None);
    };
    let newest = files(&store)
        .into_iter()
        .filter(|file| file.id == id)
        .max_by(|left, right| {
            left.times
                .mtime
                .cmp(&right.times.mtime)
                .then_with(|| left.prefix.cmp(&right.prefix))
        });
    Ok(newest.as_ref().map(pi_row))
}

fn pi_row(file: &PiFile) -> StoreSession {
    let mut row = StoreSession::new(HarnessId::Pi, &file.id, file.times.mtime);
    row.created_at = Some(file.times.birth);
    if let Some(title) = tail_window(&file.path).and_then(|text| pi_title(&text)) {
        row.title = title;
    }
    row
}

#[derive(Debug, Deserialize)]
struct PiLine {
    #[serde(rename = "type")]
    kind: Option<Field>,
    name: Option<Field>,
    session_name: Option<Field>,
    message: Option<Field>,
}

#[derive(Debug, Deserialize)]
struct PiMessage {
    role: Option<Field>,
    content: Option<Field>,
}

fn named(line: &PiLine) -> Option<String> {
    let kind = text_of(line.kind.as_ref())?;
    if kind != "name" && kind != "session_name" {
        return None;
    }
    [line.name.as_ref(), line.session_name.as_ref()]
        .into_iter()
        .find_map(|field| text_of(field).filter(|name| !name.trim().is_empty()))
        .map(|name| clip(&name, TITLE_CHARS))
}

fn user_turn(line: &PiLine) -> Option<String> {
    if !line.kind.as_ref()?.is_text("message") {
        return None;
    }
    let message = line.message.as_ref()?.object::<PiMessage>()?;
    if !message.role?.is_text("user") {
        return None;
    }
    let content = message.content?;
    let joined = match content.text() {
        Some(text) => non_empty(text.trim().to_owned())
            .into_iter()
            .collect::<Vec<_>>(),
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
            .filter_map(|text| non_empty(text.trim().to_owned()))
            .collect(),
    }
    .join("\n");
    let text = human_turn(&joined).unwrap_or(joined);
    non_empty(clip(&text, TITLE_CHARS))
}

pub(crate) fn pi_title(text: &str) -> Option<String> {
    let lines: Vec<PiLine> = text.lines().filter_map(object_line::<PiLine>).collect();
    lines
        .iter()
        .find_map(named)
        .or_else(|| lines.iter().find_map(user_turn))
}
