use std::collections::BTreeSet;

use super::Relation;
use crate::error::Error;

#[derive(Debug)]
pub(super) struct Block {
    pub(super) relation: Relation,
    pub(super) columns: Vec<String>,
    pub(super) data: Vec<u8>,
}

#[derive(Debug)]
pub(super) struct Parsed {
    pub(super) source: Option<String>,
    pub(super) host: Option<String>,
    pub(super) blocks: Vec<Block>,
}

pub(super) fn parse(dump: &str) -> Result<Parsed, Error> {
    let mut parsed = Parsed {
        source: None,
        host: None,
        blocks: Vec::new(),
    };
    let mut active: Option<Block> = None;
    let mut seen = BTreeSet::new();
    let mut complete = false;
    for (index, raw) in dump.lines().enumerate() {
        let line = match raw.strip_suffix('\r') {
            Some(stripped) => stripped,
            None => raw,
        };
        let number = index + 1;
        if active.is_some() {
            if line == "\\." {
                let Some(block) = active.take() else {
                    return Err(Error::DumpFormat {
                        line: number,
                        detail: "COPY block ended twice".into(),
                    });
                };
                parsed.blocks.push(block);
                continue;
            }
            let Some(block) = active.as_mut() else {
                return Err(Error::DumpFormat {
                    line: number,
                    detail: "COPY block disappeared".into(),
                });
            };
            block.data.extend_from_slice(line.as_bytes());
            block.data.push(b'\n');
            continue;
        }
        if line.starts_with("COPY ") {
            if complete {
                return Err(Error::DumpFormat {
                    line: number,
                    detail: "COPY after dump completion".into(),
                });
            }
            let (relation, columns) = copy_header(line, number)?;
            if !seen.insert(relation) {
                return Err(Error::DumpDuplicate {
                    table: relation.rh(),
                });
            }
            active = Some(Block {
                relation,
                columns,
                data: Vec::new(),
            });
        } else if line == "-- PostgreSQL database dump complete" {
            complete = true;
        } else {
            envelope(line, number)?;
            note(&mut parsed, line);
        }
    }
    if active.is_some() || !complete || parsed.blocks.is_empty() {
        return Err(Error::DumpTruncated);
    }
    Ok(parsed)
}

fn note(parsed: &mut Parsed, line: &str) {
    if parsed.source.is_none()
        && let Some(value) = line.strip_prefix("-- source ")
    {
        parsed.source = Some(value.trim().to_owned());
    } else if parsed.host.is_none()
        && let Some(value) = line.strip_prefix("-- host ")
    {
        parsed.host = Some(value.trim().to_owned());
    }
}

fn copy_header(text: &str, line: usize) -> Result<(Relation, Vec<String>), Error> {
    let body = text
        .strip_prefix("COPY ")
        .and_then(|body| body.strip_suffix(") FROM stdin;"))
        .ok_or_else(|| format_error(line, "expected COPY table (columns) FROM stdin;"))?;
    let (table, fields) = body
        .split_once(" (")
        .ok_or_else(|| format_error(line, "missing COPY column list"))?;
    let table = table
        .strip_prefix("public.")
        .or_else(|| table.strip_prefix("\"public\"."))
        .ok_or_else(|| format_error(line, "COPY must name the public schema"))?;
    let name = identifier(table, line)?;
    let relation = Relation::from_compat(name)
        .ok_or_else(|| format_error(line, format!("unsupported memory table {name}")))?;
    let mut columns = Vec::new();
    for field in fields.split(", ") {
        columns.push(identifier(field, line)?.to_owned());
    }
    if columns.is_empty() {
        return Err(format_error(line, "COPY column list is empty"));
    }
    Ok((relation, columns))
}

fn identifier(text: &str, line: usize) -> Result<&str, Error> {
    let text = match text.strip_prefix('"') {
        Some(quoted) => quoted
            .strip_suffix('"')
            .ok_or_else(|| format_error(line, "unclosed identifier"))?,
        None => text,
    };
    if text.is_empty()
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    {
        return Err(format_error(line, "unsupported identifier"));
    }
    Ok(text)
}

fn envelope(text: &str, line: usize) -> Result<(), Error> {
    if text.is_empty() || text.starts_with("--") {
        return Ok(());
    }
    if let Some(token) = text
        .strip_prefix("\\restrict ")
        .or_else(|| text.strip_prefix("\\unrestrict "))
        && !token.is_empty()
        && token.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        return Ok(());
    }
    if text == "SELECT pg_catalog.set_config('search_path', '', false);" {
        return Ok(());
    }
    if let Some(setting) = text
        .strip_prefix("SET ")
        .and_then(|text| text.strip_suffix(';'))
        && let Some((key, value)) = setting.split_once(" = ")
        && supported(key, value)
    {
        return Ok(());
    }
    Err(format_error(
        line,
        "unsupported statement; use pg_dump --data-only --format=plain with the supported memory tables",
    ))
}

fn supported(key: &str, value: &str) -> bool {
    match key {
        "statement_timeout"
        | "lock_timeout"
        | "idle_in_transaction_session_timeout"
        | "transaction_timeout" => value == "0",
        "client_encoding" => value == "'UTF8'",
        "standard_conforming_strings" => value == "on",
        "check_function_bodies" | "row_security" => value == "false" || value == "off",
        "client_min_messages" => value == "warning",
        "xmloption" => value == "content",
        _ => false,
    }
}

fn format_error(line: usize, detail: impl Into<String>) -> Error {
    Error::DumpFormat {
        line,
        detail: detail.into(),
    }
}
