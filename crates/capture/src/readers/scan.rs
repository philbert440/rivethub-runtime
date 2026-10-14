use std::path::Path;

use kernel::transcript::TranscriptReaderError;

use super::message::line_number;
use crate::store_path::Child;
use crate::store_path::Root;
use crate::transcript::read_capped;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RawLine {
    pub number: u64,
    pub text: String,
}

pub(crate) fn lines(text: &str) -> Result<(Vec<RawLine>, u64), TranscriptReaderError> {
    let mut records = Vec::new();
    let mut last = 0;
    for (index, line) in text.lines().enumerate() {
        let number = line_number(index.saturating_add(1))?;
        last = number;
        if line.trim().is_empty() {
            continue;
        }
        records.push(RawLine {
            number,
            text: line.to_owned(),
        });
    }
    Ok((records, last))
}

pub(crate) fn read_optional(
    root: &Root,
    path: &Path,
) -> Result<Option<String>, TranscriptReaderError> {
    match root.classify(path)? {
        Child::File => Ok(Some(read_capped(path)?)),
        Child::Absent => Ok(None),
        Child::Directory | Child::Ignored => {
            Err(crate::store_path::refused(path, "is not a regular file"))
        }
    }
}
