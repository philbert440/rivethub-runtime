use std::collections::BTreeMap;
use std::collections::BTreeSet;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::memory::DeletedSession;
use kernel::memory::MemoryStoreError;
use kernel::memory::SessionActivity;
use kernel::memory::SessionDeletion;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use wire::session_id::SessionId;

use crate::SqliteStore;
use crate::error::SqliteError;
use crate::time;

const SESSION_CONVERSATIONS: &str = "SELECT id FROM ros_conversations WHERE session_key = ?1";
const SESSION_SUMMARIES: &str = "SELECT id FROM ros_summaries WHERE conversation_id IN (SELECT id FROM ros_conversations WHERE session_key = ?1)";

fn count(value: i64) -> Result<u64, SqliteError> {
    u64::try_from(value).map_err(SqliteError::Count)
}

fn usize_count(value: usize) -> Result<u64, SqliteError> {
    u64::try_from(value).map_err(SqliteError::Count)
}

pub(crate) fn tombstone_at(
    conn: &Connection,
    session_key: &str,
) -> Result<Option<DateTime<Utc>>, SqliteError> {
    let stored: Option<String> = conn
        .query_row(
            "SELECT deleted_at FROM rh_session_tombstones WHERE session_key = ?1",
            [session_key],
            |row| row.get(0),
        )
        .optional()
        .map_err(SqliteError::query("tombstone lookup"))?;
    stored
        .map(|text| time::stored(&text, "rh_session_tombstones.deleted_at"))
        .transpose()
}

pub(crate) fn lift(conn: &Connection, session_key: &str) -> Result<(), SqliteError> {
    conn.execute(
        "DELETE FROM rh_session_tombstones WHERE session_key = ?1",
        [session_key],
    )
    .map_err(SqliteError::query("tombstone lift"))?;
    tracing::info!(
        session_key,
        "deleted session was used again; tombstone lifted"
    );
    Ok(())
}

pub(crate) fn admits(
    conn: &Connection,
    session_key: &str,
    stamps: impl IntoIterator<Item = Option<DateTime<Utc>>>,
) -> Result<bool, SqliteError> {
    let Some(deleted_at) = tombstone_at(conn, session_key)? else {
        return Ok(true);
    };
    if kernel::memory::used_since(deleted_at, stamps) {
        lift(conn, session_key)?;
        return Ok(true);
    }
    Ok(false)
}

fn used_after(
    conn: &Connection,
    session_key: &str,
    deleted_at: DateTime<Utc>,
) -> Result<bool, SqliteError> {
    conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM ros_messages m
               JOIN ros_conversations c ON c.id = m.conversation_id
              WHERE c.session_key = ?1 AND julianday(m.created_at) > julianday(?2)
         )",
        params![session_key, time::stamp(deleted_at)],
        |row| row.get(0),
    )
    .map_err(SqliteError::query("tombstone use check"))
}

fn purge(conn: &Connection, session_key: &str) -> Result<DeletedSession, SqliteError> {
    conn.execute(
        &format!(
            "DELETE FROM ros_tags
              WHERE (entity_type = 'conversation' AND entity_id IN ({SESSION_CONVERSATIONS}))
                 OR (entity_type = 'summary' AND entity_id IN ({SESSION_SUMMARIES}))"
        ),
        [session_key],
    )
    .map_err(SqliteError::query("session tags delete"))?;
    let messages: i64 = conn
        .query_row(
            "SELECT count(*) FROM ros_messages
              WHERE conversation_id IN (SELECT id FROM ros_conversations WHERE session_key = ?1)",
            [session_key],
            |row| row.get(0),
        )
        .map_err(SqliteError::query("session messages count"))?;
    let summaries = conn
        .execute(
            &format!("DELETE FROM ros_summaries WHERE id IN ({SESSION_SUMMARIES})"),
            [session_key],
        )
        .map_err(SqliteError::query("session summaries delete"))?;
    for (table, operation) in [
        ("rh_session_placements", "session placement delete"),
        ("rh_session_suggestions", "session suggestion delete"),
        ("rh_session_startup", "session startup delete"),
    ] {
        conn.execute(
            &format!("DELETE FROM {table} WHERE session_id = ?1"),
            [session_key],
        )
        .map_err(SqliteError::query(operation))?;
    }
    let conversations = conn
        .execute(
            "DELETE FROM ros_conversations WHERE session_key = ?1",
            [session_key],
        )
        .map_err(SqliteError::query("session conversations delete"))?;
    Ok(DeletedSession {
        conversations: usize_count(conversations)?,
        messages: count(messages)?,
        summaries: usize_count(summaries)?,
    })
}

fn erase(
    conn: &mut Connection,
    session_key: &str,
    tombstone: Option<(DateTime<Utc>, Option<&str>)>,
) -> Result<DeletedSession, SqliteError> {
    let tx = conn
        .transaction()
        .map_err(SqliteError::query("session delete begin"))?;
    if tombstone.is_none() {
        let Some(deleted_at) = tombstone_at(&tx, session_key)? else {
            return Ok(DeletedSession::default());
        };
        if used_after(&tx, session_key, deleted_at)? {
            lift(&tx, session_key)?;
            tx.commit()
                .map_err(SqliteError::query("tombstone lift commit"))?;
            return Ok(DeletedSession::default());
        }
    }
    if let Some((deleted_at, deleted_by)) = tombstone {
        tx.execute(
            "INSERT INTO rh_session_tombstones (session_key, deleted_at, deleted_by)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (session_key) DO UPDATE
                SET deleted_at = excluded.deleted_at, deleted_by = excluded.deleted_by",
            params![session_key, time::stamp(deleted_at), deleted_by],
        )
        .map_err(SqliteError::query("tombstone write"))?;
    }
    let deleted = purge(&tx, session_key)?;
    tx.commit()
        .map_err(SqliteError::query("session delete commit"))?;
    Ok(deleted)
}

fn matching(sessions: &[SessionId], found: &[String]) -> BTreeSet<SessionId> {
    sessions
        .iter()
        .filter(|session| found.iter().any(|key| key == session.as_str()))
        .cloned()
        .collect()
}

fn found_keys(conn: &Connection, sql: &str, keys: &str) -> Result<Vec<String>, SqliteError> {
    let mut statement = conn
        .prepare(sql)
        .map_err(SqliteError::query("session keys"))?;
    let rows = statement
        .query_map([keys], |row| row.get::<_, String>(0))
        .map_err(SqliteError::query("session keys"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(SqliteError::query("session keys"))
}

struct ActivityRow {
    session_key: String,
    conversations: i64,
    messages: i64,
    user_messages: i64,
    assistant_messages: i64,
    tool_rows: i64,
    attachments: i64,
    tasked: bool,
    reply: Option<String>,
}

fn activity_rows(conn: &Connection, keys: &str) -> Result<Vec<ActivityRow>, SqliteError> {
    let mut statement = conn
        .prepare(
            "SELECT c.session_key,
                    count(DISTINCT c.id),
                    count(m.id),
                    count(m.id) FILTER (WHERE m.role = 'user' AND trim(m.content, ' ' || char(9, 10, 13)) <> ''),
                    count(m.id) FILTER (WHERE m.role = 'assistant'),
                    count(m.id) FILTER (
                        WHERE m.role = 'tool' OR m.tool_name IS NOT NULL
                           OR m.tool_args IS NOT NULL OR m.tool_result IS NOT NULL
                    ),
                    count(m.id) FILTER (
                        WHERE json_type(m.metadata, '$.attachments') IS NOT NULL
                           OR json_type(m.metadata, '$.images') IS NOT NULL
                    ),
                    max(c.task_id IS NOT NULL),
                    (SELECT r.content FROM ros_messages r
                       JOIN ros_conversations rc ON rc.id = r.conversation_id
                      WHERE rc.session_key = c.session_key AND r.role = 'assistant'
                      ORDER BY r.created_at, r.id LIMIT 1)
               FROM ros_conversations c
               LEFT JOIN ros_messages m ON m.conversation_id = c.id
              WHERE c.session_key IN (SELECT value FROM json_each(?1))
              GROUP BY c.session_key",
        )
        .map_err(SqliteError::query("session activity"))?;
    let rows = statement
        .query_map([keys], |row| {
            Ok(ActivityRow {
                session_key: row.get(0)?,
                conversations: row.get(1)?,
                messages: row.get(2)?,
                user_messages: row.get(3)?,
                assistant_messages: row.get(4)?,
                tool_rows: row.get(5)?,
                attachments: row.get(6)?,
                tasked: row.get(7)?,
                reply: row.get(8)?,
            })
        })
        .map_err(SqliteError::query("session activity"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(SqliteError::query("session activity"))
}

fn activity(
    conn: &Connection,
    sessions: &[SessionId],
    keys: &str,
) -> Result<BTreeMap<SessionId, SessionActivity>, SqliteError> {
    let placed = matching(
        sessions,
        &found_keys(
            conn,
            "SELECT session_id FROM rh_session_placements
              WHERE session_id IN (SELECT value FROM json_each(?1))",
            keys,
        )?,
    );
    let cited = matching(
        sessions,
        &found_keys(
            conn,
            "SELECT DISTINCT c.session_key FROM ros_conversations c
              WHERE c.session_key IN (SELECT value FROM json_each(?1))
                AND (
                    EXISTS (
                        SELECT 1 FROM ros_summaries s
                          JOIN ros_wiki_citations w ON w.summary_id = s.id
                         WHERE s.conversation_id = c.id
                    )
                    OR EXISTS (
                        SELECT 1 FROM ros_wiki_provenance p
                         WHERE p.conversation_id = c.id
                            OR (p.source_kind = 'conversation' AND p.source_id = c.id)
                            OR (p.source_kind = 'summary' AND p.source_id IN (
                                SELECT s.id FROM ros_summaries s WHERE s.conversation_id = c.id
                            ))
                            OR (p.source_kind = 'message' AND p.source_id IN (
                                SELECT m.id FROM ros_messages m WHERE m.conversation_id = c.id
                            ))
                    )
                )",
            keys,
        )?,
    );
    let mut activity: BTreeMap<SessionId, SessionActivity> = sessions
        .iter()
        .map(|session| (session.clone(), SessionActivity::default()))
        .collect();
    for row in activity_rows(conn, keys)? {
        for session in sessions
            .iter()
            .filter(|session| session.as_str() == row.session_key)
        {
            activity.insert(
                session.clone(),
                SessionActivity {
                    conversations: count(row.conversations)?,
                    messages: count(row.messages)?,
                    user_messages: count(row.user_messages)?,
                    assistant_messages: count(row.assistant_messages)?,
                    reply: row.reply.clone(),
                    tool_rows: count(row.tool_rows)?,
                    attachments: count(row.attachments)?,
                    tasked: row.tasked,
                    placed: false,
                    cited: false,
                },
            );
        }
    }
    for (session, entry) in &mut activity {
        entry.placed = placed.contains(session);
        entry.cited = cited.contains(session);
    }
    Ok(activity)
}

fn session_keys(sessions: &[SessionId]) -> Result<String, SqliteError> {
    let keys: Vec<&str> = sessions.iter().map(SessionId::as_str).collect();
    serde_json::to_string(&keys).map_err(|source| SqliteError::Encode {
        column: "session keys",
        source,
    })
}

fn pending(conn: &Connection, limit: u32) -> Result<Vec<String>, SqliteError> {
    let mut statement = conn
        .prepare(
            "SELECT t.session_key FROM rh_session_tombstones t
              WHERE EXISTS (SELECT 1 FROM ros_conversations c WHERE c.session_key = t.session_key)
                 OR EXISTS (SELECT 1 FROM rh_session_placements p WHERE p.session_id = t.session_key)
                 OR EXISTS (SELECT 1 FROM rh_session_suggestions s WHERE s.session_id = t.session_key)
                 OR EXISTS (SELECT 1 FROM rh_session_startup u WHERE u.session_id = t.session_key)
              ORDER BY t.deleted_at, t.session_key
              LIMIT ?1",
        )
        .map_err(SqliteError::query("tombstone sweep"))?;
    let rows = statement
        .query_map([limit], |row| row.get::<_, String>(0))
        .map_err(SqliteError::query("tombstone sweep"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(SqliteError::query("tombstone sweep"))
}

#[async_trait]
impl SessionDeletion for SqliteStore {
    async fn delete_session(
        &self,
        session: &SessionId,
        deleted_by: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<DeletedSession, MemoryStoreError> {
        let session_key = session.as_str().to_owned();
        let deleted_by = deleted_by.map(str::to_owned);
        self.with_conn(move |conn| erase(conn, &session_key, Some((now, deleted_by.as_deref()))))
            .await
            .map_err(Into::into)
    }

    async fn tombstones(
        &self,
        sessions: &[SessionId],
    ) -> Result<BTreeMap<SessionId, DateTime<Utc>>, MemoryStoreError> {
        let sessions = sessions.to_vec();
        self.with_conn(move |conn| {
            let mut found = BTreeMap::new();
            for session in sessions {
                if let Some(deleted_at) = tombstone_at(conn, session.as_str())? {
                    found.insert(session, deleted_at);
                }
            }
            Ok(found)
        })
        .await
        .map_err(Into::into)
    }

    async fn session_activity(
        &self,
        sessions: &[SessionId],
    ) -> Result<BTreeMap<SessionId, SessionActivity>, MemoryStoreError> {
        let keys = session_keys(sessions)?;
        let sessions = sessions.to_vec();
        self.with_conn(move |conn| activity(conn, &sessions, &keys))
            .await
            .map_err(Into::into)
    }

    async fn purge_tombstoned(&self, limit: u32) -> Result<DeletedSession, MemoryStoreError> {
        self.with_conn(move |conn| {
            let mut total = DeletedSession::default();
            for session_key in pending(conn, limit)? {
                total += erase(conn, &session_key, None)?;
            }
            Ok(total)
        })
        .await
        .map_err(Into::into)
    }
}
