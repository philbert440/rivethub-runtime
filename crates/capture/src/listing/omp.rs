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
use super::files::head;
use super::files::newest_per_key;
use super::files::open_store;
use super::files::tail_window;
use super::text::TextElement;
use super::text::clip;
use super::text::clip_field;
use super::text::human_turn;
use super::text::non_empty;
use super::time::FileTimes;
use crate::readers::omp::omp_id;
use crate::store_path::Root;

const TITLE_CHARS: usize = 120;
const MODEL_CHARS: usize = 80;
const HEAD_WINDOW: u64 = 65_536;

#[derive(Debug, Clone)]
struct OmpFile {
    prefix: String,
    id: String,
    path: PathBuf,
    times: FileTimes,
}

fn candidate(entry: &Entry) -> Option<OmpFile> {
    if !entry.is_file() {
        return None;
    }
    let (prefix, id) = omp_id(entry.name.strip_suffix(".jsonl")?)?;
    Some(OmpFile {
        prefix,
        id,
        path: entry.path.clone(),
        times: file_times(&entry.path)?,
    })
}

fn files(store: &Root) -> Vec<OmpFile> {
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
    open_store(&root.join("sessions"))
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
    kept.retain(prompted);
    kept.sort_by_key(|item| std::cmp::Reverse(item.times.mtime));
    Ok(kept.iter().take(limit).map(omp_row).collect())
}

fn prompted(file: &OmpFile) -> bool {
    let long = std::fs::metadata(&file.path).is_ok_and(|meta| meta.len() > HEAD_WINDOW);
    long || head(&file.path, HEAD_WINDOW).is_none_or(|text| {
        text.lines()
            .filter_map(object_line::<OmpLine>)
            .any(|line| user_turn(&line).is_some())
    })
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
    Ok(newest.as_ref().map(omp_row))
}

fn omp_row(file: &OmpFile) -> StoreSession {
    let mut row = StoreSession::new(HarnessId::Omp, &file.id, file.times.mtime);
    row.created_at = Some(file.times.birth);
    let parsed = |text: String| {
        text.lines()
            .filter_map(object_line::<OmpLine>)
            .collect::<Vec<_>>()
    };
    let opening = head(&file.path, HEAD_WINDOW)
        .map(parsed)
        .unwrap_or_default();
    let lines = tail_window(&file.path).map(parsed).unwrap_or_default();
    let title = opening
        .first()
        .and_then(titled)
        .or_else(|| opening.iter().chain(&lines).find_map(user_turn));
    if let Some(title) = title {
        row.title = title;
    }
    row.model = lines
        .iter()
        .rev()
        .chain(opening.iter().rev())
        .find_map(model_of);
    row
}

#[derive(Debug, Deserialize)]
struct OmpLine {
    #[serde(rename = "type")]
    kind: Option<Field>,
    title: Option<Field>,
    model: Option<Field>,
    message: Option<Field>,
}

#[derive(Debug, Deserialize)]
struct OmpMessage {
    role: Option<Field>,
    content: Option<Field>,
}

fn titled(line: &OmpLine) -> Option<String> {
    if !line.kind.as_ref()?.is_text("title") {
        return None;
    }
    non_empty(clip_field(line.title.as_ref(), TITLE_CHARS))
}

fn model_of(line: &OmpLine) -> Option<String> {
    if !line.kind.as_ref()?.is_text("model_change") {
        return None;
    }
    non_empty(clip_field(line.model.as_ref(), MODEL_CHARS))
}

fn user_turn(line: &OmpLine) -> Option<String> {
    if !line.kind.as_ref()?.is_text("message") {
        return None;
    }
    let message = line.message.as_ref()?.object::<OmpMessage>()?;
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
