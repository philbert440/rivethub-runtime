use std::path::{Path, PathBuf};

use kernel::harness::TurnAttachment;
use serde::{Deserialize, Serialize};
use wire::gateway::sessions::{HarnessTranscriptTurn, SessionMessageRole, TranscriptAttachment};
use wire::session_id::SessionId;

use super::UploadError;
use super::disk;

pub(super) const TURNS_DIR: &str = "turns";
const KEPT_TURNS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AttachedTurn {
    text: String,
    attachments: Vec<TranscriptAttachment>,
}

impl AttachedTurn {
    pub(crate) fn new(text: &str, attachments: &[TurnAttachment]) -> Self {
        Self {
            text: text.trim().to_owned(),
            attachments: attachments.iter().map(transcript_attachment).collect(),
        }
    }

    fn matched(&self, text: &str) -> Option<String> {
        let links: Vec<String> = self
            .attachments
            .iter()
            .filter_map(|attachment| attachment.uri.as_deref())
            .map(resource_link)
            .collect();
        let kept = text
            .lines()
            .filter(|line| !links.iter().any(|link| line.trim() == link))
            .collect::<Vec<_>>()
            .join("\n");
        let kept = kept.trim();
        (kept == self.text).then(|| kept.to_owned())
    }
}

fn transcript_attachment(attachment: &TurnAttachment) -> TranscriptAttachment {
    TranscriptAttachment {
        media_type: attachment.media_type.clone(),
        name: Some(attachment.name.clone()),
        uri: Some(attachment.path.display().to_string()),
        upload_id: attachment
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_owned),
    }
}

fn resource_link(path: &str) -> String {
    let name = match path.rsplit_once('/') {
        Some((_, name)) => name,
        None => path,
    };
    format!("[@{name}](file://{path})")
}

pub(crate) fn attach(turns: &mut [HarnessTranscriptTurn], journal: &[AttachedTurn]) {
    let mut cursor = journal.len();
    for turn in turns
        .iter_mut()
        .rev()
        .filter(|turn| turn.role == SessionMessageRole::User)
    {
        let found = journal
            .iter()
            .enumerate()
            .take(cursor)
            .rev()
            .find_map(|(at, entry)| entry.matched(&turn.text).map(|text| (at, text, entry)));
        if let Some((at, text, entry)) = found {
            turn.text = text;
            turn.attachments = Some(entry.attachments.clone());
            cursor = at;
        }
    }
}

fn journal_path(root: &Path, session: &SessionId) -> Result<PathBuf, UploadError> {
    Ok(root
        .join(TURNS_DIR)
        .join(format!("{}.json", disk::sha256_hex(session.as_str())?)))
}

pub(super) fn read(root: &Path, session: &SessionId) -> Result<Vec<AttachedTurn>, UploadError> {
    let path = journal_path(root, session)?;
    if !std::fs::exists(&path)? {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&disk::read_contained(root, &path)?).map_err(|_| UploadError::Corrupt)
}

pub(super) fn append(
    root: &Path,
    session: &SessionId,
    turn: AttachedTurn,
) -> Result<(), UploadError> {
    let directory = root.join(TURNS_DIR);
    std::fs::create_dir_all(&directory)?;
    disk::contained_dir(root, &directory)?;
    let mut turns = read(root, session)?;
    turns.push(turn);
    let excess = turns.len().saturating_sub(KEPT_TURNS);
    turns.drain(..excess);
    let bytes = serde_json::to_vec(&turns).map_err(|_| UploadError::Corrupt)?;
    let mut temp = tempfile::Builder::new()
        .prefix(".upload-")
        .tempfile_in(&directory)?;
    std::io::Write::write_all(&mut temp, &bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(journal_path(root, session)?)
        .map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use kernel::harness::TurnAttachment;
    use wire::gateway::sessions::{HarnessTranscriptTurn, SessionMessageRole};

    use super::{AttachedTurn, attach};

    fn user(text: &str) -> HarnessTranscriptTurn {
        HarnessTranscriptTurn {
            role: SessionMessageRole::User,
            text: text.to_owned(),
            thinking: None,
            tools: None,
            usage: None,
            model: None,
            stop_reason: None,
            last_block: None,
            attachments: None,
            complete: false,
            compact: false,
            started_at_ms: None,
            ended_at_ms: None,
        }
    }

    fn sent(text: &str, path: &str, media_type: &str) -> AttachedTurn {
        let name = path.rsplit('/').next().expect("name");
        AttachedTurn::new(
            text,
            &[TurnAttachment {
                path: PathBuf::from(path),
                media_type: media_type.to_owned(),
                name: name.to_owned(),
            }],
        )
    }

    fn names(turns: &[HarnessTranscriptTurn]) -> Vec<Option<String>> {
        turns
            .iter()
            .map(|turn| {
                turn.attachments
                    .as_ref()
                    .and_then(|list| list.first())
                    .and_then(|attachment| attachment.upload_id.clone())
            })
            .collect()
    }

    #[test]
    fn the_latest_matching_turns_take_the_journal_in_order() {
        let mut turns = vec![user("look"), user("look"), user("again"), user("look")];
        let journal = [
            sent("look", "/u/a1.png", "image/png"),
            sent("look", "/u/a2.png", "image/png"),
        ];
        attach(&mut turns, &journal);
        assert_eq!(
            names(&turns),
            [None, Some("a1".to_owned()), None, Some("a2".to_owned())]
        );
    }

    #[test]
    fn a_resource_link_line_is_folded_into_the_attachment() {
        let mut turns = vec![
            user("read this\n[@n1.md](file:///u/n1.md)"),
            user("[@n2.md](file:///u/n2.md)"),
        ];
        let journal = [
            sent("read this", "/u/n1.md", "text/markdown"),
            sent("", "/u/n2.md", "text/markdown"),
        ];
        attach(&mut turns, &journal);
        assert_eq!(
            names(&turns),
            [Some("n1".to_owned()), Some("n2".to_owned())]
        );
        assert_eq!(
            turns.first().map(|turn| turn.text.as_str()),
            Some("read this")
        );
        assert_eq!(turns.get(1).map(|turn| turn.text.as_str()), Some(""));
    }
}
