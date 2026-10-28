use std::collections::BTreeSet;
use std::str::FromStr as _;

use async_trait::async_trait;
use kernel::topics::Decision;
use kernel::topics::DecisionId;
use kernel::topics::DecisionStatus;
use kernel::topics::DecisionStore;
use kernel::topics::DecisionText;
use kernel::topics::SyncTopicPage;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use wire::session_id::SessionId;

use super::rows::failed;
use super::rows::stored;
use super::rows::timestamp;
use super::rows::topic_id;
use super::write::enqueue_syncs;
use crate::SqliteStore;
use crate::time;

const COLUMNS: &str = "id, topic_id, text, status, superseded_by, decided_by, session_id, \
     created_at, updated_at";

struct DecisionRow {
    id: String,
    topic_id: String,
    text: String,
    status: String,
    superseded_by: Option<String>,
    decided_by: String,
    session_id: Option<String>,
    created_at: String,
    updated_at: String,
}

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DecisionRow> {
    Ok(DecisionRow {
        id: row.get(0)?,
        topic_id: row.get(1)?,
        text: row.get(2)?,
        status: row.get(3)?,
        superseded_by: row.get(4)?,
        decided_by: row.get(5)?,
        session_id: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn decision_id(text: &str) -> Result<DecisionId, TopicError> {
    DecisionId::from_str(text).map_err(|source| stored("decision id")(Box::new(source)))
}

fn decision(row: DecisionRow) -> Result<Decision, TopicError> {
    Ok(Decision {
        id: decision_id(&row.id)?,
        topic: topic_id(&row.topic_id)?,
        text: DecisionText::new(&row.text)?,
        status: DecisionStatus::from_str(&row.status)
            .map_err(|source| stored("decision status")(Box::new(source)))?,
        superseded_by: row.superseded_by.as_deref().map(decision_id).transpose()?,
        by: row.decided_by,
        session: row
            .session_id
            .map(SessionId::try_from)
            .transpose()
            .map_err(|source| stored("decision session")(Box::new(source)))?,
        created_at: timestamp(&row.created_at, "created_at")?,
        updated_at: timestamp(&row.updated_at, "updated_at")?,
    })
}

fn upsert(conn: &Connection, decision: &Decision) -> Result<(), TopicError> {
    conn.execute(
        "INSERT INTO rh_topic_decisions (id, topic_id, text, status, superseded_by, decided_by,
                                         session_id, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE
            SET text = excluded.text, status = excluded.status,
                superseded_by = excluded.superseded_by, updated_at = excluded.updated_at",
        params![
            decision.id.to_string(),
            decision.topic.to_string(),
            decision.text.as_str(),
            decision.status.to_string(),
            decision.superseded_by.map(|id| id.to_string()),
            decision.by,
            decision.session.as_ref().map(SessionId::as_str),
            time::stamp(decision.created_at),
            time::stamp(decision.updated_at)
        ],
    )
    .map_err(failed("decision write"))?;
    Ok(())
}

#[async_trait]
impl DecisionStore for SqliteStore {
    async fn decisions(&self, topic: TopicId) -> Result<Vec<Decision>, TopicError> {
        let topic = topic.to_string();
        self.topics("decision list", move |conn| {
            let rows = conn
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM rh_topic_decisions WHERE topic_id = ?1
                      ORDER BY created_at, id"
                ))
                .and_then(|mut statement| {
                    statement
                        .query_map(params![topic], read_row)?
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .map_err(failed("decision list"))?;
            rows.into_iter().map(decision).collect()
        })
        .await
    }

    async fn decision(&self, id: DecisionId) -> Result<Option<Decision>, TopicError> {
        let id = id.to_string();
        self.topics("decision read", move |conn| {
            conn.query_row(
                &format!("SELECT {COLUMNS} FROM rh_topic_decisions WHERE id = ?1"),
                params![id],
                read_row,
            )
            .optional()
            .map_err(failed("decision read"))?
            .map(decision)
            .transpose()
        })
        .await
    }

    async fn keep_decisions(&self, decisions: &[Decision]) -> Result<(), TopicError> {
        let decisions = decisions.to_vec();
        self.topics("decision write", move |conn| {
            for decision in &decisions {
                upsert(conn, decision)?;
            }
            let topics: BTreeSet<TopicId> =
                decisions.iter().map(|decision| decision.topic).collect();
            enqueue_syncs(conn, topics.into_iter().map(SyncTopicPage::topic).collect())
        })
        .await
    }
}
