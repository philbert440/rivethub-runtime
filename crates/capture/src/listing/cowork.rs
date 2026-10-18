use std::path::Path;
use std::path::PathBuf;

use kernel::transcript::StoreSession;
use serde::Deserialize;
use wire::harness::HarnessId;

use super::field::Field;
use super::field::object_line;
use super::field::text_of;
use super::files::entries;
use super::files::newest_per_key;
use super::files::open_store;
use super::files::whole_file;
use super::text::non_empty;
use super::time::floor_ms;
use super::time::parse_date;
use crate::store_path::Root;

const MAX_DEPTH: usize = 6;

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

fn rows(roots: &[PathBuf]) -> Vec<StoreSession> {
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
    let found = files.iter().filter_map(|path| cowork_row(path)).collect();
    newest_per_key(
        found,
        |row: &StoreSession| row.id.clone(),
        |row| row.updated_at,
    )
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
