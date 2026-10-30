use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::topics::DeliveredVia;
use kernel::topics::StartupContext;
use kernel::topics::StartupSection;
use kernel::topics::StartupStore;
use kernel::topics::TopicError;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use wire::session_id::SessionId;

use super::rows::failed;
use super::rows::stored;
use super::rows::topic_id;
use crate::SqliteStore;
use crate::time;

#[async_trait]
impl StartupStore for SqliteStore {
    async fn keep_startup(
        &self,
        session: &SessionId,
        context: &StartupContext,
        now: DateTime<Utc>,
    ) -> Result<(), TopicError> {
        let sections = serde_json::to_string(&context.sections)
            .map_err(|source| stored("startup sections")(Box::new(source)))?;
        let session = session.as_str().to_owned();
        let topic = context.topic.to_string();
        let tokens = context.tokens;
        let delivered = context.delivered_via.to_string();
        self.topics("startup context write", move |conn| {
            conn.execute(
                "INSERT INTO rh_session_startup
                    (session_id, topic_id, tokens, sections, delivered_via, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT (session_id) DO UPDATE
                    SET topic_id = excluded.topic_id, tokens = excluded.tokens,
                        sections = excluded.sections, delivered_via = excluded.delivered_via,
                        created_at = excluded.created_at",
                params![
                    session,
                    topic,
                    tokens,
                    sections,
                    delivered,
                    time::stamp(now)
                ],
            )
            .map_err(failed("startup context write"))?;
            Ok(())
        })
        .await
    }

    async fn startup(&self, session: &SessionId) -> Result<Option<StartupContext>, TopicError> {
        let session = session.as_str().to_owned();
        self.topics("startup context read", move |conn| {
            let row = conn
                .query_row(
                    "SELECT topic_id, tokens, sections, delivered_via
                     FROM rh_session_startup WHERE session_id = ?1",
                    params![session],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, usize>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(failed("startup context read"))?;
            let Some((topic, tokens, sections, delivered)) = row else {
                return Ok(None);
            };
            let sections: Vec<StartupSection> = serde_json::from_str(&sections)
                .map_err(|source| stored("startup sections")(Box::new(source)))?;
            Ok(Some(StartupContext {
                topic: topic_id(&topic)?,
                sections,
                tokens,
                delivered_via: delivered
                    .parse::<DeliveredVia>()
                    .map_err(|source| stored("startup delivery")(Box::new(source)))?,
            }))
        })
        .await
    }
}
