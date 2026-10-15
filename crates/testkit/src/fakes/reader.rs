use async_trait::async_trait;
use chrono::{DateTime, Utc};
use kernel::transcript::{TranscriptReader, TranscriptReaderError, Turn};
use std::{collections::BTreeMap, str::FromStr};
use wire::session_id::SessionId;
#[derive(Debug)]
pub struct FakeReader {
    turns: BTreeMap<SessionId, Vec<Turn>>,
}

impl FakeReader {
    pub fn new(turns: BTreeMap<String, Vec<Turn>>) -> Result<Self, TranscriptReaderError> {
        let turns = turns
            .into_iter()
            .map(|(key, turns)| {
                SessionId::from_str(&key)
                    .map(|session| (session, turns))
                    .map_err(|source| TranscriptReaderError::Read {
                        message: format!("invalid scripted session {key}"),
                        source: Box::new(source),
                    })
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { turns })
    }
}

#[async_trait]
impl TranscriptReader for FakeReader {
    async fn sessions(
        &self,
        _store: &std::path::Path,
    ) -> Result<Vec<SessionId>, TranscriptReaderError> {
        Ok(self.turns.keys().cloned().collect())
    }

    async fn read(&self, session: &SessionId) -> Result<Vec<Turn>, TranscriptReaderError> {
        self.turns
            .get(session)
            .cloned()
            .ok_or_else(|| TranscriptReaderError::Read {
                message: format!("no scripted session {session}"),
                source: Box::new(super::provider::MissingScript("transcript")),
            })
    }

    async fn tail(
        &self,
        session: &SessionId,
        since: DateTime<Utc>,
    ) -> Result<Vec<Turn>, TranscriptReaderError> {
        Ok(self
            .read(session)
            .await?
            .into_iter()
            .filter(|turn| turn.at > since)
            .collect())
    }
}
