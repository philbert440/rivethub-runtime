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
use super::files::head;
use super::files::open_store;
use super::text::TextElement;
use super::text::clip;
use super::text::collapse;
use super::text::non_empty;
use super::time::FileTimes;
use crate::readers::codex::codex_id;
use crate::store_path::Root;

const HEAD_WINDOW: u64 = 65_536;
const TITLE_CHARS: usize = 120;

#[derive(Debug, Clone)]
struct Rollout {
    id: String,
    path: PathBuf,
    times: FileTimes,
}

fn digits(name: &str, width: usize) -> bool {
    name.len() == width && name.bytes().all(|byte| byte.is_ascii_digit())
}

fn rollouts(store: &Root) -> Vec<Rollout> {
    let mut found = Vec::new();
    let dated = |dir: &Path, width: usize| {
        directories(store, dir)
            .into_iter()
            .filter(move |entry| digits(&entry.name, width))
    };
    for year in dated(store.canonical(), 4) {
        for month in dated(&year.path, 2) {
            for day in dated(&month.path, 2) {
                for entry in entries(store, &day.path) {
                    let id = entry.name.strip_suffix(".jsonl").and_then(codex_id);
                    if let Some(id) = id.filter(|_| entry.is_file())
                        && let Some(times) = file_times(&entry.path)
                    {
                        found.push(Rollout {
                            id,
                            path: entry.path,
                            times,
                        });
                    }
                }
            }
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
    let mut found = rollouts(&store);
    found.sort_by_key(|item| std::cmp::Reverse(item.times.mtime));
    Ok(found.iter().take(limit).map(rollout_row).collect())
}

pub(crate) fn describe(
    root: &Path,
    id: &str,
) -> Result<Option<StoreSession>, TranscriptReaderError> {
    let Some(store) = sessions(root)? else {
        return Ok(None);
    };
    let newest = rollouts(&store)
        .into_iter()
        .filter(|rollout| rollout.id == id)
        .fold(None::<Rollout>, |newest, candidate| match newest {
            Some(current) if current.times.mtime >= candidate.times.mtime => Some(current),
            Some(_) | None => Some(candidate),
        });
    Ok(newest.as_ref().map(rollout_row))
}

fn rollout_row(rollout: &Rollout) -> StoreSession {
    let mut row = StoreSession::new(HarnessId::Codex, &rollout.id, rollout.times.mtime);
    row.created_at = Some(rollout.times.birth);
    if let Some(title) = non_empty(
        collapse(&codex_title(&rollout.path))
            .chars()
            .take(TITLE_CHARS)
            .collect(),
    ) {
        row.title = title;
    }
    row
}

#[derive(Debug, Deserialize)]
struct RolloutLine {
    #[serde(rename = "type")]
    kind: Option<Field>,
    payload: Option<Field>,
}

#[derive(Debug, Deserialize)]
struct RolloutPayload {
    #[serde(rename = "type")]
    kind: Option<Field>,
    role: Option<Field>,
    content: Option<Field>,
}

fn user_message(line: &str) -> Option<String> {
    let parsed = object_line::<RolloutLine>(line)?;
    if !parsed.kind?.is_text("response_item") {
        return None;
    }
    let payload = parsed.payload?.object::<RolloutPayload>()?;
    if !payload.kind?.is_text("message") || !payload.role?.is_text("user") {
        return None;
    }
    let content = payload.content?;
    let text = match content.text() {
        Some(text) => text,
        None => content
            .items()?
            .iter()
            .filter_map(Field::object::<TextElement>)
            .filter(|element| {
                element
                    .kind
                    .as_ref()
                    .is_some_and(|kind| kind.is_text("input_text") || kind.is_text("text"))
            })
            .filter_map(|element| text_of(element.text.as_ref()))
            .collect(),
    };
    let trimmed = text.trim();
    let injected = [
        "<environment_context>",
        "<skills_instructions>",
        "<multi_agent_",
    ]
    .iter()
    .any(|prefix| trimmed.starts_with(prefix));
    (!trimmed.is_empty() && !injected).then(|| clip(trimmed, TITLE_CHARS))
}

pub(crate) fn codex_title(path: &Path) -> String {
    head(path, HEAD_WINDOW)
        .and_then(|text| text.lines().find_map(user_message))
        .unwrap_or_default()
}
