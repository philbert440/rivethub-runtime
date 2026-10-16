use kernel::memory::StoredChunk;

use crate::error::WorkerError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextChunk {
    pub text: String,
    pub char_start: u32,
    pub char_end: u32,
}

pub fn split_embed_text(text: &str, max_chars: usize) -> Result<Vec<TextChunk>, WorkerError> {
    if max_chars == 0 {
        return Err(WorkerError::ChunkWindow);
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return Ok(vec![piece(&chars, 0, chars.len())?]);
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = window_end(&chars, start, max_chars);
        let end = end.max(start.saturating_add(1)).min(chars.len());
        chunks.push(piece(&chars, start, end)?);
        start = end;
    }
    Ok(chunks)
}

pub(crate) fn stored_chunk(
    index: usize,
    piece: &TextChunk,
    vector: Option<Vec<f32>>,
) -> Result<StoredChunk, WorkerError> {
    Ok(StoredChunk {
        index: u32::try_from(index).map_err(|_| WorkerError::TextTooLong)?,
        char_start: piece.char_start,
        char_end: piece.char_end,
        content: piece.text.clone(),
        vector,
    })
}

fn window_end(chars: &[char], start: usize, max_chars: usize) -> usize {
    let hard = (start + max_chars).min(chars.len());
    if hard == chars.len() {
        return hard;
    }
    let region = start + (max_chars * 85) / 100;
    if region >= hard {
        return hard;
    }
    if let Some(at) = rfind_pair(chars, region, hard, '\n', '\n') {
        return at + 2;
    }
    if let Some(at) = rfind_char(chars, region, hard, '\n') {
        return at + 1;
    }
    if let Some(at) = rfind_pair(chars, region, hard, '.', ' ') {
        return at + 2;
    }
    hard
}

fn piece(chars: &[char], start: usize, end: usize) -> Result<TextChunk, WorkerError> {
    let text: String = chars
        .iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect();
    Ok(TextChunk {
        text,
        char_start: index(start)?,
        char_end: index(end)?,
    })
}

fn index(offset: usize) -> Result<u32, WorkerError> {
    u32::try_from(offset).map_err(|_| WorkerError::TextTooLong)
}

fn rfind_char(chars: &[char], from: usize, to: usize, needle: char) -> Option<usize> {
    chars
        .iter()
        .enumerate()
        .take(to)
        .skip(from)
        .rev()
        .find(|(_, ch)| **ch == needle)
        .map(|(index, _)| index)
}

fn rfind_pair(chars: &[char], from: usize, to: usize, first: char, second: char) -> Option<usize> {
    let last = to.saturating_sub(1);
    if last <= from {
        return None;
    }
    chars
        .iter()
        .enumerate()
        .take(last)
        .skip(from)
        .rev()
        .find_map(|(index, ch)| {
            (*ch == first && chars.get(index.saturating_add(1)) == Some(&second)).then_some(index)
        })
}
