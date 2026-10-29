use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::str::FromStr;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::filing::ConversationPlacement;
use kernel::filing::Correction;
use kernel::filing::CorrectionExample;
use kernel::filing::Filed;
use kernel::filing::FiledConversation;
use kernel::filing::FiledSession;
use kernel::filing::FilingStore;
use kernel::filing::Subject;
use kernel::filing::Target;
use kernel::filing::TopicLine;
use kernel::filing::conversation_keys;
use kernel::memory::ConversationHead;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use kernel::topics::TopicName;
use sqlx::PgConnection;
use uuid::Uuid;

use super::rows;
use super::rows::PlacementRow;
use super::rows::failed;
use super::rows::not_found;
use super::rows::stored;
use crate::PostgresMemory;

struct ConversationPlacementRow {
    conversation_id: Uuid,
    topic_id: Uuid,
    placed_by: String,
    placed_at: DateTime<Utc>,
}

struct HeadRow {
    id: Uuid,
    session_key: String,
    agent: String,
    channel: String,
    title: Option<String>,
}

struct SeenRow {
    session_key: String,
    title: Option<String>,
    last_active: Option<DateTime<Utc>>,
}

#[async_trait]
impl FilingStore for PostgresMemory {
    async fn subject(&self, conversation: Uuid) -> Result<Option<Subject>, TopicError> {
        let row = sqlx::query!(
            r#"SELECT c.id, c.session_key, c.agent, c.channel, c.title,
                      c.settings->>'cwd' AS cwd,
                      (SELECT s.content FROM rh_summaries s
                        WHERE s.conversation_id = c.id AND s.kind = 'leaf'
                        ORDER BY s.earliest_at NULLS LAST, s.created_at
                        LIMIT 1) AS summary
                 FROM rh_conversations c
                WHERE c.id = $1"#,
            conversation
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(failed("filing subject"))?;
        Ok(row.map(|row| Subject {
            head: ConversationHead {
                id: row.id,
                session_key: row.session_key,
                agent: row.agent,
                channel: row.channel,
                title: row.title,
            },
            cwd: row.cwd.filter(|cwd| !cwd.trim().is_empty()),
            summary: row.summary,
        }))
    }

    async fn task_parent(
        &self,
        task: Uuid,
        conversation: Uuid,
    ) -> Result<Option<ConversationHead>, TopicError> {
        let named = format!("%{task}%");
        let row = sqlx::query_as!(
            HeadRow,
            "SELECT c.id, c.session_key, c.agent, c.channel, c.title
               FROM rh_messages m
               JOIN rh_conversations c ON c.id = m.conversation_id
               JOIN rh_conversations own ON own.id = $2
              WHERE m.conversation_id <> $2
                AND m.created_at BETWEEN own.created_at - interval '1 day'
                                     AND own.created_at + interval '1 day'
                AND (m.tool_result LIKE $1 OR m.content LIKE $1)
              ORDER BY m.created_at
              LIMIT 1",
            named,
            conversation
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(failed("task parent"))?;
        Ok(row.map(|row| ConversationHead {
            id: row.id,
            session_key: row.session_key,
            agent: row.agent,
            channel: row.channel,
            title: row.title,
        }))
    }

    async fn conversation_placement(
        &self,
        conversation: Uuid,
    ) -> Result<Option<ConversationPlacement>, TopicError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(failed("conversation placement read"))?;
        placement_of(&mut conn, conversation, false).await
    }

    async fn place_conversation(
        &self,
        placement: ConversationPlacement,
    ) -> Result<ConversationPlacement, TopicError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(failed("conversation placement"))?;
        if !super::write::topic_exists(&mut tx, placement.topic).await? {
            return Err(not_found(placement.topic));
        }
        let existing = placement_of(&mut tx, placement.conversation, true).await?;
        let kept = placement.clone().admit(existing);
        if kept == placement {
            sqlx::query!(
                "INSERT INTO rh_conversation_placements (conversation_id, topic_id, placed_by, placed_at)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (conversation_id) DO UPDATE SET topic_id = EXCLUDED.topic_id,
                                                             placed_by = EXCLUDED.placed_by,
                                                             placed_at = EXCLUDED.placed_at",
                placement.conversation,
                placement.topic.uuid(),
                placement.by.to_string(),
                placement.at
            )
            .execute(&mut *tx)
            .await
            .map_err(failed("conversation placement write"))?;
        }
        tx.commit()
            .await
            .map_err(failed("conversation placement"))?;
        Ok(kept)
    }

    async fn unplace_conversation(
        &self,
        conversation: Uuid,
    ) -> Result<Option<ConversationPlacement>, TopicError> {
        sqlx::query_as!(
            ConversationPlacementRow,
            "DELETE FROM rh_conversation_placements WHERE conversation_id = $1
             RETURNING conversation_id, topic_id, placed_by, placed_at",
            conversation
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(failed("conversation unplace"))?
        .map(|row| conversation_placement(&row))
        .transpose()
    }

    async fn filed(&self, topic: TopicId) -> Result<Filed, TopicError> {
        let placed = sqlx::query_as!(
            PlacementRow,
            "SELECT session_id, topic_id, placed_by, placed_at
               FROM rh_session_placements WHERE topic_id = $1",
            topic.uuid()
        )
        .fetch_all(&self.pool)
        .await
        .map_err(failed("filed sessions"))?
        .into_iter()
        .map(rows::placement)
        .collect::<Result<Vec<Placement>, _>>()?;
        let keys: Vec<String> = placed
            .iter()
            .flat_map(|placement| conversation_keys(&placement.session))
            .collect();
        let seen: BTreeMap<String, SeenRow> = sqlx::query_as!(
            SeenRow,
            r#"SELECT DISTINCT ON (session_key) session_key, title,
                      max(updated_at) OVER (PARTITION BY session_key) AS last_active
                 FROM rh_conversations
                WHERE session_key = ANY($1::text[])
                ORDER BY session_key, title IS NULL, updated_at DESC"#,
            &keys
        )
        .fetch_all(&self.pool)
        .await
        .map_err(failed("filed session titles"))?
        .into_iter()
        .map(|row| (row.session_key.clone(), row))
        .collect();
        let mut sessions: Vec<FiledSession> = placed
            .into_iter()
            .map(|placement| {
                let found: Vec<&SeenRow> = conversation_keys(&placement.session)
                    .iter()
                    .filter_map(|key| seen.get(key))
                    .collect();
                FiledSession {
                    title: found.iter().find_map(|row| row.title.clone()),
                    last_active: found.iter().filter_map(|row| row.last_active).max(),
                    placement,
                }
            })
            .collect();
        sessions.sort_by_key(|filed| Reverse(filed.last_active));
        let mut conversations = sqlx::query!(
            "SELECT p.conversation_id, p.topic_id, p.placed_by, p.placed_at,
                    c.title, c.channel, c.updated_at
               FROM rh_conversation_placements p
               JOIN rh_conversations c ON c.id = p.conversation_id
              WHERE p.topic_id = $1",
            topic.uuid()
        )
        .fetch_all(&self.pool)
        .await
        .map_err(failed("filed conversations"))?
        .into_iter()
        .map(|row| {
            Ok(FiledConversation {
                placement: conversation_placement(&ConversationPlacementRow {
                    conversation_id: row.conversation_id,
                    topic_id: row.topic_id,
                    placed_by: row.placed_by,
                    placed_at: row.placed_at,
                })?,
                title: row.title,
                channel: row.channel,
                last_active: Some(row.updated_at),
            })
        })
        .collect::<Result<Vec<_>, TopicError>>()?;
        conversations.sort_by_key(|filed| Reverse(filed.last_active));
        Ok(Filed {
            sessions,
            conversations,
        })
    }

    async fn correct(&self, correction: Correction) -> Result<(), TopicError> {
        let (session, conversation) = match &correction.target {
            Target::Session(session) => (Some(session.as_str().to_owned()), None),
            Target::Conversation(conversation) => (None, Some(*conversation)),
        };
        sqlx::query!(
            "INSERT INTO rh_filing_corrections
                 (id, session_id, conversation_id, from_topic, to_topic, corrected_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
            Uuid::new_v4(),
            session,
            conversation,
            correction.from.uuid(),
            correction.to.map(TopicId::uuid),
            correction.at
        )
        .execute(&self.pool)
        .await
        .map_err(failed("filing correction"))?;
        Ok(())
    }

    async fn corrections(&self, limit: u32) -> Result<Vec<CorrectionExample>, TopicError> {
        sqlx::query!(
            r#"SELECT f.name AS from_name, t.name AS "to_name?",
                      COALESCE(
                          (SELECT c.title FROM rh_conversations c WHERE c.id = k.conversation_id),
                          (SELECT c.title FROM rh_conversations c
                            WHERE c.session_key = k.session_id
                            ORDER BY c.updated_at DESC LIMIT 1)
                      ) AS title
                 FROM rh_filing_corrections k
                 JOIN rh_topics f ON f.id = k.from_topic
                 LEFT JOIN rh_topics t ON t.id = k.to_topic
                ORDER BY k.corrected_at DESC
                LIMIT $1"#,
            i64::from(limit)
        )
        .fetch_all(&self.pool)
        .await
        .map_err(failed("filing corrections"))?
        .into_iter()
        .map(|row| {
            Ok(CorrectionExample {
                title: row.title,
                from: TopicName::new(&row.from_name)?,
                to: row.to_name.as_deref().map(TopicName::new).transpose()?,
            })
        })
        .collect()
    }

    async fn topic_lines(&self) -> Result<Vec<TopicLine>, TopicError> {
        Ok(sqlx::query!(
            r#"SELECT topic_id AS "topic_id!", current_state
                 FROM rh_wiki_topics WHERE topic_id IS NOT NULL"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(failed("topic lines"))?
        .into_iter()
        .filter_map(|row| {
            TopicLine::from_page(TopicId::from_uuid(row.topic_id), &row.current_state)
        })
        .collect())
    }
}

pub(super) async fn fold_into(
    conn: &mut PgConnection,
    source: Uuid,
    into: Uuid,
) -> Result<(), TopicError> {
    sqlx::query!(
        "WITH placed AS (
             UPDATE rh_conversation_placements SET topic_id = $2 WHERE topic_id = $1
         )
         UPDATE rh_session_suggestions SET parent_id = $2 WHERE parent_id = $1",
        source,
        into
    )
    .execute(&mut *conn)
    .await
    .map_err(failed("topic merge filings"))?;
    Ok(())
}

async fn placement_of(
    conn: &mut PgConnection,
    conversation: Uuid,
    lock: bool,
) -> Result<Option<ConversationPlacement>, TopicError> {
    let row = if lock {
        sqlx::query_as!(
            ConversationPlacementRow,
            "SELECT conversation_id, topic_id, placed_by, placed_at
               FROM rh_conversation_placements WHERE conversation_id = $1 FOR UPDATE",
            conversation
        )
        .fetch_optional(&mut *conn)
        .await
    } else {
        sqlx::query_as!(
            ConversationPlacementRow,
            "SELECT conversation_id, topic_id, placed_by, placed_at
               FROM rh_conversation_placements WHERE conversation_id = $1",
            conversation
        )
        .fetch_optional(&mut *conn)
        .await
    };
    row.map_err(failed("conversation placement read"))?
        .map(|row| conversation_placement(&row))
        .transpose()
}

fn conversation_placement(
    row: &ConversationPlacementRow,
) -> Result<ConversationPlacement, TopicError> {
    Ok(ConversationPlacement {
        conversation: row.conversation_id,
        topic: TopicId::from_uuid(row.topic_id),
        by: PlacementSource::from_str(&row.placed_by)
            .map_err(|error| stored("placed_by")(Box::new(error)))?,
        at: row.placed_at,
    })
}
