use std::path::Path;

use kernel::memory::DeletedSession;
use kernel::memory::SessionDeletion;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::TopicStore;
use memory_sqlite::SqliteStore;
use rusqlite::Connection;
use tempfile::TempDir;
use testkit::session_delete;
use testkit::topics::at;
use testkit::topics::new_topic;
use wire::session_id::SessionId;

use crate::common::TestResult;

fn store() -> TestResult<(TempDir, SqliteStore)> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("memory.sqlite");
    migrate::sqlite(&path)?;
    let store = SqliteStore::open(&path)?;
    Ok((directory, store))
}

fn connection(directory: &TempDir) -> TestResult<Connection> {
    let conn = Connection::open(directory.path().join("memory.sqlite"))?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;
    Ok(conn)
}

fn conversations(conn: &Connection, session: &SessionId) -> TestResult<Vec<String>> {
    let mut statement =
        conn.prepare("SELECT id FROM ros_conversations WHERE session_key = ?1 ORDER BY agent")?;
    let ids = statement
        .query_map([session.as_str()], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

fn seed_related(conn: &Connection, session: &SessionId, tag: &str) -> TestResult {
    let stamp = "2026-10-27T12:00:00.000Z";
    for (index, conversation) in conversations(conn, session)?.iter().enumerate() {
        let summary = format!("{tag}-summary-{index}");
        conn.execute(
            "INSERT INTO ros_summaries (id, conversation_id, content, created_at)
             VALUES (?1, ?2, 'summary text', ?3)",
            rusqlite::params![summary, conversation, stamp],
        )?;
        conn.execute(
            "INSERT INTO ros_summary_sources (summary_id, message_id, ordinal)
             SELECT ?1, id, 0 FROM ros_messages WHERE conversation_id = ?2",
            rusqlite::params![summary, conversation],
        )?;
        for (entity_type, entity) in [("conversation", conversation), ("summary", &summary)] {
            conn.execute(
                "INSERT INTO ros_tags (id, entity_type, entity_id, key, value, source, state, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'project', 'demo', 'user', 'accepted', ?4, ?4)",
                rusqlite::params![format!("{tag}-{entity_type}-{index}"), entity_type, entity, stamp],
            )?;
        }
    }
    conn.execute(
        "INSERT INTO rh_session_startup (session_id, topic_id, tokens, sections, created_at)
         VALUES (?1, 'topic', 0, '[]', ?2)",
        rusqlite::params![session.as_str(), stamp],
    )?;
    conn.execute(
        "INSERT INTO rh_session_history (session_id, marked_at) VALUES (?1, ?2)",
        rusqlite::params![session.as_str(), stamp],
    )?;
    Ok(())
}

fn count(conn: &Connection, sql: &str, session: &SessionId) -> TestResult<i64> {
    Ok(conn.query_row(sql, [session.as_str()], |row| row.get(0))?)
}

fn remaining(conn: &Connection, session: &SessionId) -> TestResult<[i64; 8]> {
    let of_conversations = "SELECT id FROM ros_conversations WHERE session_key = ?1";
    let of_summaries =
        format!("SELECT id FROM ros_summaries WHERE conversation_id IN ({of_conversations})");
    Ok([
        count(
            conn,
            "SELECT count(*) FROM ros_conversations WHERE session_key = ?1",
            session,
        )?,
        count(
            conn,
            &format!(
                "SELECT count(*) FROM ros_messages WHERE conversation_id IN ({of_conversations})"
            ),
            session,
        )?,
        count(
            conn,
            &format!("SELECT count(*) FROM ({of_summaries})"),
            session,
        )?,
        count(
            conn,
            &format!(
                "SELECT count(*) FROM ros_summary_sources WHERE summary_id IN ({of_summaries})"
            ),
            session,
        )?,
        count(
            conn,
            &format!(
                "SELECT count(*) FROM ros_tags WHERE entity_id IN ({of_conversations}) OR entity_id IN ({of_summaries})"
            ),
            session,
        )?,
        count(
            conn,
            "SELECT count(*) FROM rh_session_placements WHERE session_id = ?1",
            session,
        )?,
        count(
            conn,
            "SELECT count(*) FROM rh_session_startup WHERE session_id = ?1",
            session,
        )?,
        count(
            conn,
            "SELECT count(*) FROM rh_session_history WHERE session_id = ?1",
            session,
        )?,
    ])
}

async fn place(store: &SqliteStore, sessions: &[&SessionId]) -> TestResult {
    let topic = store.create(new_topic("Release", &[], None)?).await?;
    for session in sessions {
        store
            .place(Placement {
                session: (*session).clone(),
                topic: topic.id,
                by: PlacementSource::You,
                at: at(3),
            })
            .await?;
    }
    Ok(())
}

#[tokio::test]
async fn delete_removes_one_session_and_leaves_its_sibling() -> TestResult {
    let (directory, store) = store()?;
    let (deleted, sibling) = session_delete::seed(&store).await?;
    place(&store, &[&deleted, &sibling]).await?;
    let conn = connection(&directory)?;
    seed_related(&conn, &deleted, "d")?;
    seed_related(&conn, &sibling, "s")?;
    let sibling_before = remaining(&conn, &sibling)?;
    assert_eq!(remaining(&conn, &deleted)?, [2, 4, 2, 4, 4, 1, 1, 1]);
    let erased = store
        .delete_session(deleted.as_str(), Some("user-1"), at(10))
        .await?;
    assert_eq!(
        erased,
        DeletedSession {
            conversations: 2,
            messages: 4,
            summaries: 2,
        }
    );
    assert_eq!(remaining(&conn, &deleted)?, [0; 8]);
    assert_eq!(remaining(&conn, &sibling)?, sibling_before);
    let tombstone: (String, Option<String>) = conn.query_row(
        "SELECT deleted_at, deleted_by FROM rh_session_tombstones WHERE session_key = ?1",
        [deleted.as_str()],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(tombstone.1.as_deref(), Some("user-1"));
    Ok(())
}

#[tokio::test]
async fn a_session_without_memory_rows_still_gets_a_tombstone() -> TestResult {
    let (directory, store) = store()?;
    let session = testkit::topics::session("00000000-0000-4000-8000-0000000000e1")?;
    let erased = store.delete_session(session.as_str(), None, at(1)).await?;
    assert_eq!(erased, DeletedSession::default());
    assert_eq!(
        store.tombstones(&[session.as_str().to_owned()]).await?,
        std::collections::BTreeMap::from([(session.as_str().to_owned(), at(1))])
    );
    let conn = connection(&directory)?;
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM rh_session_tombstones WHERE session_key = ?1",
            &session
        )?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn capture_and_append_for_a_deleted_session_write_nothing() -> TestResult {
    let (_directory, store) = store()?;
    session_delete::capture_for_a_deleted_session_writes_nothing(&store, &store).await?;
    Ok(())
}

#[tokio::test]
async fn an_empty_session_is_reported_until_it_is_deleted() -> TestResult {
    let (_directory, store) = store()?;
    session_delete::an_empty_session_is_reported_until_it_is_deleted(&store, &store).await?;
    Ok(())
}

#[tokio::test]
async fn activity_counts_turns_tools_and_placements() -> TestResult {
    let (_directory, store) = store()?;
    session_delete::activity_counts_turns_tools_and_placements(&store, &store, &store).await?;
    Ok(())
}

#[tokio::test]
async fn a_summary_cited_by_a_wiki_page_marks_the_session_cited() -> TestResult {
    let (directory, store) = store()?;
    let (deleted, sibling) = session_delete::seed(&store).await?;
    let conn = connection(&directory)?;
    seed_related(&conn, &sibling, "s")?;
    conn.execute(
        "INSERT INTO ros_wiki_topics (slug, title, created_at, updated_at)
         VALUES ('release', 'Release', '2026-10-27T12:00:00.000Z', '2026-10-27T12:00:00.000Z')",
        [],
    )?;
    conn.execute(
        "INSERT INTO ros_wiki_citations (topic_slug, summary_id, cited_at)
         VALUES ('release', 's-summary-0', '2026-10-27T12:00:00.000Z')",
        [],
    )?;
    let activity = store
        .session_activity(&[deleted.as_str().to_owned(), sibling.as_str().to_owned()])
        .await?;
    assert_eq!(
        activity.get(sibling.as_str()).map(|entry| entry.cited),
        Some(true)
    );
    assert_eq!(
        activity.get(deleted.as_str()).map(|entry| entry.cited),
        Some(false)
    );
    Ok(())
}

fn insert_behind(path: &Path, session: &SessionId, created_at: &str) -> TestResult {
    let conn = Connection::open(path)?;
    conn.execute(
        "INSERT INTO ros_conversations (id, session_key, agent, created_at, updated_at)
         VALUES ('behind', ?1, 'external', '2026-10-27T12:00:00.000Z', '2026-10-27T12:00:00.000Z')",
        [session.as_str()],
    )?;
    conn.execute(
        "INSERT INTO ros_messages (id, conversation_id, agent, channel, role, content, created_at)
         VALUES ('behind-1', 'behind', 'external', 'cli', 'user', 'written directly', ?1)",
        [created_at],
    )?;
    Ok(())
}

#[tokio::test]
async fn the_sweep_removes_rows_written_behind_the_gateway() -> TestResult {
    let (directory, store) = store()?;
    let (deleted, sibling) = session_delete::seed(&store).await?;
    store.delete_session(deleted.as_str(), None, at(10)).await?;
    insert_behind(
        &directory.path().join("memory.sqlite"),
        &deleted,
        "2026-09-01T12:00:00.000Z",
    )?;
    let purged = store.purge_tombstoned(100).await?;
    assert_eq!(
        purged,
        DeletedSession {
            conversations: 1,
            messages: 1,
            summaries: 0,
        }
    );
    assert_eq!(
        store.purge_tombstoned(100).await?,
        DeletedSession::default()
    );
    let conn = connection(&directory)?;
    assert_eq!(remaining(&conn, &deleted)?, [0; 8]);
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM ros_conversations WHERE session_key = ?1",
            &sibling
        )?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn the_sweep_lifts_the_tombstone_when_the_session_was_used_again() -> TestResult {
    let (directory, store) = store()?;
    let (deleted, _sibling) = session_delete::seed(&store).await?;
    store.delete_session(deleted.as_str(), None, at(10)).await?;
    insert_behind(
        &directory.path().join("memory.sqlite"),
        &deleted,
        "2026-12-01T12:00:00.000Z",
    )?;
    assert_eq!(
        store.purge_tombstoned(100).await?,
        DeletedSession::default()
    );
    assert!(
        store
            .tombstones(&[deleted.as_str().to_owned()])
            .await?
            .is_empty()
    );
    let conn = connection(&directory)?;
    assert_eq!(
        count(
            &conn,
            "SELECT count(*) FROM ros_messages m
               JOIN ros_conversations c ON c.id = m.conversation_id
              WHERE c.session_key = ?1",
            &deleted
        )?,
        1
    );
    Ok(())
}

#[tokio::test]
async fn a_deleted_session_used_again_comes_back() -> TestResult {
    let (_directory, store) = store()?;
    session_delete::a_deleted_session_used_again_comes_back(&store, &store).await?;
    Ok(())
}

#[tokio::test]
async fn a_kept_session_is_remembered_from_its_latest_keep() -> TestResult {
    let (_directory, store) = store()?;
    session_delete::a_kept_session_is_remembered_from_its_latest_keep(&store).await?;
    Ok(())
}
