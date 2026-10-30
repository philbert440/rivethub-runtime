use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::topics::DeliveredVia;
use kernel::topics::StartupContext;
use kernel::topics::StartupSection;
use kernel::topics::StartupStore;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use wire::session_id::SessionId;

use super::rows::failed;
use super::rows::stored;
use crate::PostgresMemory;

#[async_trait]
impl StartupStore for PostgresMemory {
    async fn keep_startup(
        &self,
        session: &SessionId,
        context: &StartupContext,
        now: DateTime<Utc>,
    ) -> Result<(), TopicError> {
        let sections = serde_json::to_value(&context.sections)
            .map_err(|source| stored("startup sections")(Box::new(source)))?;
        let tokens = i32::try_from(context.tokens)
            .map_err(|source| stored("startup tokens")(Box::new(source)))?;
        sqlx::query!(
            "INSERT INTO rh_session_startup
                (session_id, topic_id, tokens, sections, delivered_via, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (session_id) DO UPDATE
                SET topic_id = EXCLUDED.topic_id, tokens = EXCLUDED.tokens,
                    sections = EXCLUDED.sections, delivered_via = EXCLUDED.delivered_via,
                    created_at = EXCLUDED.created_at",
            session.as_str(),
            context.topic.uuid(),
            tokens,
            sections,
            context.delivered_via.to_string(),
            now
        )
        .execute(&self.pool)
        .await
        .map_err(failed("startup context write"))?;
        Ok(())
    }

    async fn startup(&self, session: &SessionId) -> Result<Option<StartupContext>, TopicError> {
        let Some(row) = sqlx::query!(
            "SELECT topic_id, tokens, sections, delivered_via
             FROM rh_session_startup WHERE session_id = $1",
            session.as_str()
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(failed("startup context read"))?
        else {
            return Ok(None);
        };
        let sections: Vec<StartupSection> = serde_json::from_value(row.sections)
            .map_err(|source| stored("startup sections")(Box::new(source)))?;
        Ok(Some(StartupContext {
            topic: TopicId::from_uuid(row.topic_id),
            sections,
            tokens: usize::try_from(row.tokens)
                .map_err(|source| stored("startup tokens")(Box::new(source)))?,
            delivered_via: row
                .delivered_via
                .parse::<DeliveredVia>()
                .map_err(|source| stored("startup delivery")(Box::new(source)))?,
        }))
    }
}
