use std::str::FromStr as _;

use kernel::filing::conversation_keys;
use kernel::tidy::Proposal;
use kernel::tidy::TidyError;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use wire::session_id::SessionId;

use super::failed;
use super::stored;

pub(super) fn current_titles(
    conn: &Connection,
    proposals: &mut [Proposal],
) -> Result<(), TidyError> {
    let mut statement = conn
        .prepare(
            "SELECT title FROM ros_conversations
              WHERE (session_key IN (SELECT value FROM json_each(?1)) OR id = ?2)
                AND title IS NOT NULL
              ORDER BY updated_at DESC LIMIT 1",
        )
        .map_err(failed("tidy sample titles"))?;
    for sample in proposals
        .iter_mut()
        .flat_map(|proposal| &mut proposal.evidence.samples)
    {
        let keys = if let Ok(session) = SessionId::from_str(&sample.id) {
            conversation_keys(&session)
        } else {
            vec![sample.id.clone()]
        };
        let keys = serde_json::to_string(&keys).map_err(stored("keys"))?;
        let title: Option<String> = statement
            .query_row(params![keys, sample.id], |row| row.get(0))
            .optional()
            .map_err(failed("tidy sample titles"))?;
        if title.is_some() {
            sample.title = title;
        }
    }
    Ok(())
}
