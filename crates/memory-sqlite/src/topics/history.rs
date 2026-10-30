use kernel::topics::HistoryMark;
use kernel::topics::TopicError;
use rusqlite::Connection;
use rusqlite::params;
use wire::session_id::SessionId;

use super::rows::failed;
use super::rows::stored;
use super::rows::timestamp;
use crate::time;

pub(super) fn mark(session_id: String, marked_at: &str) -> Result<HistoryMark, TopicError> {
    Ok(HistoryMark {
        session: SessionId::try_from(session_id)
            .map_err(|error| stored("session_id")(Box::new(error)))?,
        at: timestamp(marked_at, "marked_at")?,
    })
}

pub(super) fn keep(conn: &Connection, kept: &HistoryMark) -> Result<HistoryMark, TopicError> {
    let (session_id, marked_at) = conn
        .query_row(
            "INSERT INTO rh_session_history (session_id, marked_at) VALUES (?1, ?2)
             ON CONFLICT (session_id) DO UPDATE SET marked_at = excluded.marked_at
             RETURNING session_id, marked_at",
            params![kept.session.as_str(), time::stamp(kept.at)],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(failed("session history mark"))?;
    mark(session_id, &marked_at)
}

pub(super) fn forget(conn: &Connection, session: &SessionId) -> Result<(), TopicError> {
    conn.execute(
        "DELETE FROM rh_session_history WHERE session_id = ?1",
        params![session.as_str()],
    )
    .map_err(failed("session history unmark"))?;
    Ok(())
}

pub(super) fn marks(conn: &Connection) -> Result<Vec<HistoryMark>, TopicError> {
    conn.prepare(
        "SELECT session_id, marked_at FROM rh_session_history
          ORDER BY marked_at DESC, session_id",
    )
    .and_then(|mut statement| {
        statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()
    })
    .map_err(failed("session history"))?
    .into_iter()
    .map(|(session_id, marked_at)| mark(session_id, &marked_at))
    .collect()
}
