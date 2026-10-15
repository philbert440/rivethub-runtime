use std::path::Path;

use regex::Captures;
use regex::Regex;
use serde::Deserialize;

use crate::Error;
use crate::Mismatch;
use crate::json::Json;
use crate::json::{Masks, json_masked_eq, masked_eq};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookPost {
    pub url: String,
    pub content_type: String,
    pub body: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookFile {
    pub path: String,
    pub mode: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct HookOutcome {
    pub exit: i64,
    pub stdout: String,
    pub stderr: String,
    pub posts: Vec<HookPost>,
    pub files: Vec<HookFile>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EntryRecord {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub stdin: String,
    #[serde(default)]
    pub exit: i64,
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    #[serde(default)]
    pub posts: Vec<PostRecord>,
    #[serde(default)]
    pub files: Vec<FileRecord>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PostRecord {
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub url: String,
    #[serde(rename = "contentType", default)]
    pub content_type: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FileRecord {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub content: String,
}

pub const ISO_TIMESTAMP: &str = "[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(?:[.][0-9]+)?(?:Z|[+-][0-9]{2}:[0-9]{2})";

pub fn entries(recording: &Path) -> Result<Vec<EntryRecord>, Error> {
    let raw = crate::read_text(recording)?;
    crate::parse_json(recording, &raw)
}

pub fn replay(
    entry: &EntryRecord,
    run_hook: impl FnOnce(&str, &str) -> HookOutcome,
) -> Result<(), Mismatch> {
    let id = format!("hook/{}", entry.index);
    let actual = run_hook(&entry.event, &entry.stdin);
    if actual.exit != entry.exit {
        return Err(Mismatch::new(
            &id,
            format!("exit {} != recorded {}", actual.exit, entry.exit),
        ));
    }
    if actual.stdout != entry.stdout {
        return Err(Mismatch::new(&id, "stdout mismatch"));
    }
    if actual.stderr != entry.stderr {
        return Err(Mismatch::new(&id, "stderr mismatch"));
    }
    compare_posts(&id, entry, &actual)?;
    compare_files(&id, entry, &actual)
}

fn mask_capture_text(text: &str) -> Result<String, Mismatch> {
    Ok(Regex::new(&format!(
        "({ISO_TIMESTAMP})|([0-9]{{13}}-[0-9a-z]{{6,}})|(<ts>)"
    ))?
    .replace_all(text, |caps: &Captures<'_>| {
        if caps.get(2).is_some() {
            "{{KEY}}"
        } else {
            "<ts>"
        }
    })
    .into_owned())
}

fn compare_posts(id: &str, entry: &EntryRecord, actual: &HookOutcome) -> Result<(), Mismatch> {
    if entry.posts.len() != actual.posts.len() {
        return Err(Mismatch::new(
            id,
            format!(
                "{} post(s), recorded {}",
                actual.posts.len(),
                entry.posts.len()
            ),
        ));
    }
    for (index, (want, got)) in entry.posts.iter().zip(&actual.posts).enumerate() {
        compare_post(id, index, want, got)?;
    }
    Ok(())
}

fn compare_post(id: &str, index: usize, want: &PostRecord, got: &HookPost) -> Result<(), Mismatch> {
    if want.method != "POST" {
        return Err(Mismatch::new(
            id,
            format!("post {index}: recorded method is not POST"),
        ));
    }
    if want.url != got.url {
        return Err(Mismatch::new(id, format!("post {index}: url mismatch")));
    }
    if want.content_type != got.content_type {
        return Err(Mismatch::new(
            id,
            format!("post {index}: content type mismatch"),
        ));
    }
    let Ok(mut want_body) = serde_json::from_str(&want.body) else {
        return Err(Mismatch::new(
            id,
            format!("post {index}: recorded body unreadable"),
        ));
    };
    let Ok(got_body) = serde_json::from_str(&got.body) else {
        return Err(Mismatch::new(
            id,
            format!("post {index}: actual body is not JSON"),
        ));
    };
    mask_value(&mut want_body)?;
    if !json_masked_eq(&want_body, &got_body, &Masks::default())? {
        return Err(Mismatch::new(
            id,
            format!(
                "post {index}: batch body mismatch\n    recorded: {want_body}\n    actual:   {got_body}"
            ),
        ));
    }
    Ok(())
}

fn compare_files(id: &str, entry: &EntryRecord, actual: &HookOutcome) -> Result<(), Mismatch> {
    if entry.files.len() != actual.files.len() {
        return Err(Mismatch::new(
            id,
            format!(
                "{} file(s), recorded {}",
                actual.files.len(),
                entry.files.len()
            ),
        ));
    }
    for (index, (want, got)) in entry.files.iter().zip(&actual.files).enumerate() {
        if want.path != got.path {
            return Err(Mismatch::new(id, format!("file {index}: path mismatch")));
        }
        if want.mode != got.mode {
            return Err(Mismatch::new(id, format!("file {index}: mode mismatch")));
        }
        let want_content = mask_capture_text(&want.content)?;
        if !masked_eq(&want_content, &got.content, &Masks::default())? {
            return Err(Mismatch::new(id, format!("file {index}: content mismatch")));
        }
    }
    Ok(())
}

fn mask_value(value: &mut Json) -> Result<(), Mismatch> {
    match value {
        Json::Object(map) => {
            for member in map.values_mut() {
                mask_value(member)?;
            }
        }
        Json::Array(items) => {
            for item in items {
                mask_value(item)?;
            }
        }
        Json::String(text) => {
            *text = mask_capture_text(text)?;
        }
        Json::Null | Json::Bool(_) | Json::Number(_) => {}
    }
    Ok(())
}
