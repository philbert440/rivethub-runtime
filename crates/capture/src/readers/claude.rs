use kernel::transcript::TranscriptReaderError;
use wire::message::Role;
use wire::session_id::SessionId;

use super::message::Mapped;
use super::message::SkippedRecord;
use super::message::TextTurn;
use super::message::ToolFields;
use super::message::TranscriptSlice;
use super::message::assemble;
use super::message::line_number;
use super::message::push_mapped;
use super::message::text_turn;
use super::message::tool_turn;
use super::message::unknown;
use crate::transcript::Line;
use crate::transcript::LineKind;
use crate::transcript::Parsed;

const FORMAT: &str = "claude-jsonl";

pub(crate) fn from_parsed(
    session: &SessionId,
    parsed: &Parsed,
    offset: u64,
) -> Result<TranscriptSlice, TranscriptReaderError> {
    let mut drafts = Vec::new();
    let mut skipped = Vec::new();
    let mut last = offset;
    for line in &parsed.skipped {
        let position = line_number(line.number)?;
        last = last.max(position);
        if position > offset {
            skipped.push(SkippedRecord {
                position,
                rule: super::message::SkipRule::UnparseableRecord,
                detail: line.reason.clone(),
            });
        }
    }
    for line in &parsed.lines {
        let position = line_number(line.number)?;
        last = last.max(position);
        push_mapped(
            map_line(session, &parsed.lines, line, position)?,
            &mut drafts,
            &mut skipped,
        );
    }
    Ok(assemble(offset, last, drafts, skipped))
}

fn map_line(
    session: &SessionId,
    lines: &[Line],
    line: &Line,
    position: u64,
) -> Result<Mapped, TranscriptReaderError> {
    if line.is_meta == Some(true) {
        return Ok(Mapped::None);
    }
    if line.r#type == LineKind::Other {
        return Ok(Mapped::Skip(unknown(position, "unknown record type")));
    }
    let Some(at) = line.timestamp.as_deref() else {
        return Ok(Mapped::None);
    };
    let Some((role, content)) = turn_parts(line) else {
        return Ok(Mapped::None);
    };
    let turn = TextTurn {
        session,
        position,
        role,
        content: &content,
        at,
        format: FORMAT,
    };
    if role == Role::Tool {
        tool_turn(&turn, tool_fields(lines, line))
    } else {
        text_turn(&turn)
    }
}

fn turn_parts(line: &Line) -> Option<(Role, String)> {
    match line.r#type {
        LineKind::User => match line.tool_result_text() {
            Some(result) => Some((Role::Tool, result)),
            None => line.text().map(|text| (Role::User, text)),
        },
        LineKind::Assistant => line.text().map(|text| (Role::Assistant, text)),
        LineKind::Other => None,
    }
}

fn tool_fields(lines: &[Line], line: &Line) -> ToolFields {
    let Some(outcome) = line.tool_results().into_iter().next() else {
        return ToolFields::none();
    };
    let matched = outcome.tool_use_id.as_deref().and_then(|id| {
        lines.iter().find_map(|candidate| {
            candidate
                .tool_uses()
                .into_iter()
                .find(|tool| tool.id == id)
                .map(|tool| (tool.name.to_owned(), tool.input.arguments().clone()))
        })
    });
    let (name, arguments) = matched.map_or((None, None), |(name, arguments)| {
        (Some(name), Some(arguments))
    });
    ToolFields {
        name,
        arguments,
        result: Some(outcome.text),
    }
}
