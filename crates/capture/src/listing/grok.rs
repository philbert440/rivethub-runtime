use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fs;
use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::path::PathBuf;

use kernel::transcript::SessionRenameError;
use kernel::transcript::StoreSession;
use kernel::transcript::TranscriptReaderError;
use parking_lot::Mutex;
use serde::Deserialize;
use serde::Serialize;
use wire::harness::HarnessId;

use super::ancestors::with_ancestors;
use super::field::Field;
use super::field::first_present;
use super::field::object_line;
use super::field::text_of;
use super::files::complete_lines;
use super::files::directories;
use super::files::open_store;
use super::files::regular_file;
use super::files::unreadable;
use super::files::whole_file;
use super::files::write_failure;
use super::text::clip_field;
use super::text::is_uuid;
use super::text::non_empty;
use super::text::segment_safe;
use super::time::UNKNOWN_TIME;
use super::time::parse_date;
use super::time::system_ms;
use super::title::HarnessTitles;
use crate::store_path::Root;

const SPAWN_MARKER: &str = "subagent_spawned";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cursor {
    device: u64,
    inode: u64,
    size: u64,
    mtime: i64,
    parsed_until: u64,
    children: Vec<(String, String)>,
}

#[derive(Debug, Default)]
pub(crate) struct SpawnIndex {
    cursors: Mutex<HashMap<PathBuf, Cursor>>,
}

impl SpawnIndex {
    fn parents(&self, files: &[PathBuf]) -> HashMap<String, String> {
        let mut cursors = self.cursors.lock();
        cursors.retain(|path, _| files.contains(path));
        let mut parents = HashMap::new();
        for file in files {
            let Some(cursor) = refreshed(cursors.remove(file), file) else {
                continue;
            };
            for (child, parent) in &cursor.children {
                parents.insert(child.clone(), parent.clone());
            }
            cursors.insert(file.clone(), cursor);
        }
        parents
    }
}

fn refreshed(previous: Option<Cursor>, path: &Path) -> Option<Cursor> {
    match advance(previous, path) {
        Ok(cursor) => Some(cursor),
        Err(error) => {
            unreadable(path, &error);
            None
        }
    }
}

fn advance(previous: Option<Cursor>, path: &Path) -> io::Result<Cursor> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    let mtime = system_ms(metadata.modified()?);
    let (device, inode, size) = (metadata.dev(), metadata.ino(), metadata.len());
    let mut cursor = match previous {
        Some(cursor)
            if cursor.device == device && cursor.inode == inode && size >= cursor.parsed_until =>
        {
            cursor
        }
        Some(_) | None => Cursor {
            device,
            inode,
            size: 0,
            mtime: 0,
            parsed_until: 0,
            children: Vec::new(),
        },
    };
    if cursor.size == size && cursor.mtime == mtime {
        return Ok(cursor);
    }
    file.seek(SeekFrom::Start(cursor.parsed_until))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let complete = complete_lines(&bytes);
    for line in String::from_utf8_lossy(complete).lines() {
        if line.contains(SPAWN_MARKER)
            && let Some(link) = spawn_link(line)
        {
            cursor.children.push(link);
        }
    }
    cursor.parsed_until += complete.len() as u64;
    cursor.size = size;
    cursor.mtime = mtime;
    Ok(cursor)
}

#[derive(Debug, Deserialize)]
struct SpawnLine {
    params: Option<Field>,
}

#[derive(Debug, Deserialize)]
struct SpawnParams {
    update: Option<Field>,
}

#[derive(Debug, Deserialize)]
struct SpawnUpdate {
    #[serde(rename = "sessionUpdate")]
    session_update: Option<Field>,
    child_session_id: Option<Field>,
    subagent_id: Option<Field>,
    parent_session_id: Option<Field>,
}

fn spawn_link(line: &str) -> Option<(String, String)> {
    let update = object_line::<SpawnLine>(line)?
        .params?
        .object::<SpawnParams>()?
        .update?
        .object::<SpawnUpdate>()?;
    if !update.session_update.as_ref()?.is_text(SPAWN_MARKER) {
        return None;
    }
    let child = first_present(&[
        update.child_session_id.as_ref(),
        update.subagent_id.as_ref(),
    ])?
    .text()
    .map(|child| child.trim().to_owned())
    .filter(|child| is_uuid(child))?;
    let parent = text_of(update.parent_session_id.as_ref())
        .map(|parent| parent.trim().to_owned())
        .filter(|parent| is_uuid(parent))?;
    (child != parent).then_some((child, parent))
}

#[derive(Debug, Deserialize)]
struct SummaryFile {
    info: Option<Field>,
    session_summary: Option<Field>,
    updated_at: Option<Field>,
    created_at: Option<Field>,
    agent_name: Option<Field>,
    current_model_id: Option<Field>,
    session_kind: Option<Field>,
    generated_title: Option<Field>,
    title_is_manual: Option<Field>,
}

impl SummaryFile {
    fn titles(&self) -> HarnessTitles {
        let mut titles = HarnessTitles::default();
        if let Some(generated) = text_of(self.generated_title.as_ref()) {
            if self.title_is_manual.as_ref().is_some_and(Field::flag) {
                titles.manual(&generated);
            } else {
                titles.automatic(&generated);
            }
        }
        titles
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct SummaryTitle {
    generated_title: Option<String>,
    #[serde(default)]
    title_is_manual: bool,
    #[serde(flatten)]
    rest: wire::Extra,
}

#[derive(Debug, Deserialize)]
struct Info {
    id: Option<Field>,
}

#[derive(Debug, Clone)]
struct GrokRow {
    row: StoreSession,
    nested: bool,
    untitled: bool,
}

struct Scan {
    rows: Vec<GrokRow>,
    updates: Vec<PathBuf>,
}

fn scan(store: &Root, only: Option<&BTreeSet<&str>>) -> Scan {
    let mut scan = Scan {
        rows: Vec::new(),
        updates: Vec::new(),
    };
    for encoded in directories(store, store.canonical()) {
        for session in directories(store, &encoded.path) {
            let updates = session.path.join("updates.jsonl");
            if regular_file(store, &updates) {
                scan.updates.push(updates);
            }
            if only.is_some_and(|ids| !ids.contains(session.name.as_str())) {
                continue;
            }
            let summary = session.path.join("summary.json");
            if !regular_file(store, &summary) {
                continue;
            }
            if let Some(row) =
                whole_file(&summary).and_then(|text| summary_row(&text, &session.name))
                && (only.is_some()
                    || !row.untitled
                    || user_turn(&session.path.join("chat_history.jsonl")))
            {
                scan.rows.push(row);
            }
        }
    }
    scan
}

#[derive(Debug, Deserialize)]
struct HistoryLine {
    #[serde(rename = "type")]
    kind: Option<Field>,
    synthetic_reason: Option<Field>,
}

fn user_turn(history: &Path) -> bool {
    whole_file(history).is_some_and(|text| {
        text.lines()
            .filter_map(object_line::<HistoryLine>)
            .any(|line| {
                line.synthetic_reason.is_none()
                    && line.kind.as_ref().is_some_and(|kind| kind.is_text("user"))
            })
    })
}

fn summary_row(text: &str, dir_name: &str) -> Option<GrokRow> {
    let summary = object_line::<SummaryFile>(text)?;
    let id = summary
        .info
        .as_ref()
        .and_then(Field::object::<Info>)
        .and_then(|info| text_of(info.id.as_ref()))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| dir_name.to_owned());
    let updated_at = text_of(summary.updated_at.as_ref())
        .and_then(|text| parse_date(&text))
        .unwrap_or(UNKNOWN_TIME);
    let mut row = StoreSession::new(HarnessId::GrokBuild, &id, updated_at);
    let opening = non_empty(clip_field(summary.session_summary.as_ref(), 120));
    let untitled = opening.is_none()
        && text_of(summary.generated_title.as_ref()).is_none_or(|title| title.trim().is_empty());
    if let Some(title) = opening {
        row.title = title;
    }
    summary.titles().apply(&mut row);
    row.created_at = text_of(summary.created_at.as_ref()).and_then(|text| parse_date(&text));
    row.agent_name = non_empty(clip_field(summary.agent_name.as_ref(), 64));
    row.model = non_empty(clip_field(summary.current_model_id.as_ref(), 80));
    let nested = text_of(summary.session_kind.as_ref())
        .is_some_and(|kind| kind == "subagent" || kind == "subagent_fork");
    Some(GrokRow {
        row,
        nested,
        untitled,
    })
}

fn stamp(rows: Vec<GrokRow>, parents: &HashMap<String, String>) -> Vec<StoreSession> {
    rows.into_iter()
        .map(|grok| {
            let mut row = grok.row;
            if grok.nested {
                row.parent_session_id = parents
                    .get(&row.id)
                    .filter(|parent| **parent != row.id)
                    .cloned();
            }
            row
        })
        .collect()
}

fn sessions(root: &Path) -> Result<Option<Root>, TranscriptReaderError> {
    open_store(&root.join("sessions"))
}

pub(crate) fn list(
    root: &Path,
    spawns: &SpawnIndex,
    limit: usize,
) -> Result<Vec<StoreSession>, TranscriptReaderError> {
    let Some(store) = sessions(root)? else {
        return Ok(Vec::new());
    };
    let scan = scan(&store, None);
    let mut rows = stamp(scan.rows, &spawns.parents(&scan.updates));
    rows.sort_by_key(|item| std::cmp::Reverse(item.updated_at));
    Ok(with_ancestors(rows, limit))
}

pub(crate) fn describe(
    root: &Path,
    spawns: &SpawnIndex,
    id: &str,
) -> Result<Option<StoreSession>, TranscriptReaderError> {
    Ok(describe_many(root, spawns, &[id.to_owned()])?.pop())
}

pub(crate) fn describe_many(
    root: &Path,
    spawns: &SpawnIndex,
    ids: &[String],
) -> Result<Vec<StoreSession>, TranscriptReaderError> {
    let wanted: BTreeSet<&str> = ids
        .iter()
        .map(String::as_str)
        .filter(|id| !id.is_empty() && !id.contains('/') && !id.contains(".."))
        .collect();
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    let Some(store) = sessions(root)? else {
        return Ok(Vec::new());
    };
    let scan = scan(&store, Some(&wanted));
    let mut newest: BTreeMap<String, StoreSession> = BTreeMap::new();
    for row in stamp(scan.rows, &spawns.parents(&scan.updates)) {
        match newest.get(&row.id) {
            Some(current) if current.updated_at >= row.updated_at => {}
            Some(_) | None => {
                newest.insert(row.id.clone(), row);
            }
        }
    }
    Ok(newest.into_values().collect())
}

pub(crate) fn title(root: &Path, id: &str) -> Result<Option<String>, TranscriptReaderError> {
    if !segment_safe(id) {
        return Ok(None);
    }
    let Some(store) = sessions(root)? else {
        return Ok(None);
    };
    Ok(directories(&store, store.canonical())
        .into_iter()
        .map(|encoded| encoded.path.join(id).join("summary.json"))
        .filter(|summary| regular_file(&store, summary))
        .filter_map(|summary| whole_file(&summary).and_then(|text| summary_row(&text, id)))
        .max_by_key(|grok| grok.row.updated_at)
        .filter(|grok| !grok.untitled)
        .map(|grok| grok.row.title))
}

pub(crate) fn rename(
    root: &Path,
    spawns: &SpawnIndex,
    id: &str,
    title: &str,
) -> Result<Option<StoreSession>, SessionRenameError> {
    if !segment_safe(id) {
        return Ok(None);
    }
    let Some(store) = sessions(root)? else {
        return Ok(None);
    };
    let Some(summary) = directories(&store, store.canonical())
        .into_iter()
        .map(|encoded| encoded.path.join(id).join("summary.json"))
        .find(|summary| regular_file(&store, summary))
    else {
        return Ok(None);
    };
    retitle(&summary, title).map_err(|error| write_failure(&summary, error))?;
    Ok(describe(root, spawns, id)?)
}

fn retitle(summary: &Path, title: &str) -> io::Result<()> {
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(summary.with_extension("json.lock"))?;
    lock.lock()?;
    let mut document: SummaryTitle = serde_json::from_slice(&fs::read(summary)?)?;
    document.generated_title = Some(title.to_owned());
    document.title_is_manual = true;
    let staged = summary.with_extension("json.rivethub");
    fs::write(&staged, serde_json::to_vec_pretty(&document)?)?;
    fs::rename(&staged, summary)?;
    lock.unlock()
}
