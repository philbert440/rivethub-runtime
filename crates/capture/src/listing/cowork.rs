use std::path::Path;
use std::path::PathBuf;

use kernel::transcript::StoreSession;
use serde::Deserialize;
use wire::harness::HarnessId;

use super::field::Field;
use super::field::object_line;
use super::field::text_of;
use super::files::entries;
use super::files::file_times;
use super::files::newest_per_key;
use super::files::open_store;
use super::files::whole_file;
use super::text::non_empty;
use super::time::floor_ms;
use super::time::parse_date;
use crate::store_path::Root;

const MAX_DEPTH: usize = 6;
const TASK_PREFIX: usize = 8;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CoworkMeta {
    cli_session_id: Option<Field>,
    created_at: Option<Field>,
    #[serde(rename = "created_at")]
    snake_created_at: Option<Field>,
    last_activity_at: Option<Field>,
    updated_at: Option<Field>,
    title: Option<Field>,
    cwd: Option<Field>,
}

pub(crate) fn list(roots: &[PathBuf], limit: usize) -> Vec<StoreSession> {
    let mut rows = rows(roots);
    rows.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
    rows.truncate(limit);
    rows
}

pub(crate) fn describe(roots: &[PathBuf], id: &str) -> Option<StoreSession> {
    rows(roots).into_iter().find(|row| row.id == id)
}

pub(crate) fn transcripts(roots: &[PathBuf]) -> Vec<(String, PathBuf)> {
    tasks(roots)
        .into_iter()
        .filter_map(|(row, meta)| transcript_of(&row, &meta).map(|transcript| (row.id, transcript)))
        .collect()
}

fn rows(roots: &[PathBuf]) -> Vec<StoreSession> {
    tasks(roots).into_iter().map(|(row, _)| row).collect()
}

fn tasks(roots: &[PathBuf]) -> Vec<(StoreSession, PathBuf)> {
    let mut seen: Vec<&PathBuf> = Vec::new();
    let mut files = Vec::new();
    for root in roots {
        if seen.contains(&root) {
            continue;
        }
        seen.push(root);
        for name in ["local-agent-mode-sessions", "claude-code-sessions"] {
            let dir = root.join(name);
            match open_store(&dir) {
                Ok(Some(store)) => walk(&store, store.canonical(), 0, &mut files),
                Ok(None) => {}
                Err(error) => {
                    tracing::debug!(%error, dir = %dir.display(), "cowork session directory is refused and is skipped");
                }
            }
        }
    }
    let found = files
        .into_iter()
        .filter_map(|path| cowork_row(&path).map(|row| (row, path)))
        .collect();
    newest_per_key(
        found,
        |(row, _): &(StoreSession, PathBuf)| row.id.clone(),
        |(row, _)| row.updated_at,
    )
}

fn transcript_of(row: &StoreSession, meta: &Path) -> Option<PathBuf> {
    let name = format!("{}.jsonl", row.id);
    task_directories(meta, row.cwd.as_deref())
        .into_iter()
        .find_map(|dir| newest_named(&dir.join(".claude").join("projects"), &name))
}

fn task_directories(meta: &Path, cwd: Option<&str>) -> Vec<PathBuf> {
    let (Some(dir), Some(stem)) = (
        meta.parent(),
        meta.file_stem().and_then(|stem| stem.to_str()),
    ) else {
        return Vec::new();
    };
    let mut found = vec![dir.join(stem)];
    if let Some(parent) = cwd.map(Path::new).and_then(Path::parent)
        && parent.parent() == Some(dir)
    {
        found.push(parent.to_path_buf());
    }
    if let Some(prefix) = stem
        .strip_prefix("local_")
        .and_then(|task| task.get(..TASK_PREFIX))
    {
        found.push(dir.join(prefix));
    }
    found.dedup();
    found
}

fn newest_named(projects: &Path, name: &str) -> Option<PathBuf> {
    let store = match open_store(projects) {
        Ok(store) => store?,
        Err(error) => {
            tracing::debug!(%error, dir = %projects.display(), "cowork projects directory is refused and is skipped");
            return None;
        }
    };
    let mut found = Vec::new();
    named(&store, store.canonical(), name, 0, &mut found);
    found
        .into_iter()
        .filter_map(|path| file_times(&path).map(|times| (times.mtime, path)))
        .max_by_key(|(mtime, _)| *mtime)
        .map(|(_, path)| path)
}

fn named(store: &Root, dir: &Path, name: &str, depth: usize, found: &mut Vec<PathBuf>) {
    for entry in entries(store, dir) {
        if entry.is_dir() && depth < MAX_DEPTH {
            named(store, &entry.path, name, depth + 1, found);
        } else if entry.is_file() && entry.name == name {
            found.push(entry.path);
        }
    }
}

fn walk(store: &Root, dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    for entry in entries(store, dir) {
        if entry.hidden() {
            continue;
        }
        if entry.is_dir() && depth < MAX_DEPTH {
            walk(store, &entry.path, depth + 1, files);
        } else if entry.is_file()
            && entry.name.starts_with("local_")
            && entry
                .path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            files.push(entry.path);
        }
    }
}

fn cowork_row(path: &Path) -> Option<StoreSession> {
    let meta = whole_file(path).and_then(|text| object_line::<CoworkMeta>(&text))?;
    let id = text_of(meta.cli_session_id.as_ref())
        .map(|id| id.trim().to_owned())
        .filter(|id| {
            !id.is_empty() && !id.contains('/') && !id.contains("..") && !id.contains('\\')
        })?;
    let created = ms(meta.created_at.as_ref()).or_else(|| ms(meta.snake_created_at.as_ref()))?;
    let updated = ms(meta.last_activity_at.as_ref())
        .or_else(|| ms(meta.updated_at.as_ref()))
        .unwrap_or(created);
    let mut row = StoreSession::new(HarnessId::Cowork, &id, updated);
    row.created_at = Some(created);
    if let Some(title) =
        text_of(meta.title.as_ref()).and_then(|title| non_empty(title.trim().to_owned()))
    {
        row.title = title;
    }
    row.cwd = text_of(meta.cwd.as_ref()).and_then(|cwd| non_empty(cwd.trim().to_owned()));
    Some(row)
}

fn ms(field: Option<&Field>) -> Option<i64> {
    let field = field?;
    if let Some(number) = field.number() {
        return number.is_finite().then(|| floor_ms(number));
    }
    let text = field.text()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(number) = trimmed.parse::<f64>()
        && number.is_finite()
    {
        return Some(floor_ms(number));
    }
    parse_date(trimmed)
}
