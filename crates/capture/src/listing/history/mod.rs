mod claude;
mod tools;

use std::path::Path;
use std::path::PathBuf;

use kernel::transcript::StoreTranscript;
use kernel::transcript::TranscriptRead;
use kernel::transcript::TranscriptReader;
use kernel::transcript::TranscriptReaderError;
use kernel::transcript::Turn;
use wire::gateway::sessions::HarnessTranscriptTurn;
use wire::gateway::sessions::SessionMessageRole;
use wire::harness::HarnessId;
use wire::message::Role;
use wire::session_id::SessionId;

use super::StoreRoots;
use crate::readers::StoreReader;
use crate::readers::blocking;

pub(super) async fn transcript(
    roots: &StoreRoots,
    harness: HarnessId,
    native: &str,
) -> Result<TranscriptRead, TranscriptReaderError> {
    if harness == HarnessId::Cowork {
        return Ok(TranscriptRead::NoSource);
    }
    let Some(root) = roots.single(harness).map(Path::to_path_buf) else {
        return Ok(TranscriptRead::Read(StoreTranscript::default()));
    };
    if harness == HarnessId::ClaudeCode {
        let native = native.to_owned();
        return blocking(move || claude::transcript(&root, &native))
            .await
            .map(TranscriptRead::Read);
    }
    text_turns(harness, root, native).await
}

async fn text_turns(
    harness: HarnessId,
    root: PathBuf,
    native: &str,
) -> Result<TranscriptRead, TranscriptReaderError> {
    let Some(reader) = StoreReader::open(harness, root) else {
        return Ok(TranscriptRead::NoSource);
    };
    let turns = reader.read(&SessionId::new(harness, native)).await?;
    Ok(TranscriptRead::Read(StoreTranscript {
        turns: turns.into_iter().filter_map(text_turn).collect(),
        truncated: false,
    }))
}

fn text_turn(turn: Turn) -> Option<HarnessTranscriptTurn> {
    let role = match turn.role {
        Role::User => SessionMessageRole::User,
        Role::Assistant => SessionMessageRole::Assistant,
        Role::System | Role::Tool => return None,
    };
    Some(HarnessTranscriptTurn {
        role,
        text: turn.content,
        thinking: None,
        tools: None,
        usage: None,
        model: None,
        stop_reason: None,
        last_block: None,
        complete: false,
        compact: false,
    })
}
