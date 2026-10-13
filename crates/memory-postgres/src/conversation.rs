use crate::capture::CaptureWriter;
use crate::capture::ConversationEnsure;
use crate::capture::ConversationRecord;
use crate::capture::ConversationUpsert;
use crate::error::Error;
use uuid::Uuid;

impl CaptureWriter<'_> {
    pub async fn ensure_conversation(
        &mut self,
        ensure: &ConversationEnsure<'_>,
    ) -> Result<ConversationRecord, Error> {
        let found = sqlx::query!(
            "select id, title from rh_conversations
              where session_key = $1 and agent = $2 limit 1",
            ensure.session_key,
            ensure.agent
        )
        .fetch_optional(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("conversation lookup", source))?;
        if let Some(row) = found {
            if let Some(task_id) = ensure.task_id {
                self.backfill_task_id(row.id, task_id).await?;
            }
            return Ok(ConversationRecord {
                id: row.id,
                created: false,
                title: row.title,
            });
        }
        let settings = serde_json::to_value(ensure.settings)
            .map_err(|source| Error::protocol(source.to_string()))?;
        let inserted = sqlx::query!(
            "insert into rh_conversations
               (session_key, agent, channel, title, settings, active, created_at, updated_at,
                task_id, owner_user_id)
             values ($1, $2, $3, $4, $5, $6, coalesce($7, now()), coalesce($8, now()),
                     $9::text::uuid, $10)
             returning id",
            ensure.session_key,
            ensure.agent,
            ensure.channel,
            ensure.title,
            settings,
            ensure.active,
            ensure.created_at,
            ensure.updated_at,
            ensure.task_id,
            ensure.owner_user_id
        )
        .fetch_one(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("conversation insert", source))?;
        Ok(ConversationRecord {
            id: inserted.id,
            created: true,
            title: None,
        })
    }

    pub async fn upsert_conversation(
        &mut self,
        upsert: &ConversationUpsert<'_>,
    ) -> Result<Uuid, Error> {
        let settings = upsert
            .settings
            .map(serde_json::to_value)
            .transpose()
            .map_err(|source| Error::protocol(source.to_string()))?;
        let record = sqlx::query!(
            "insert into rh_conversations (session_key, agent, channel, title, settings,
                                           task_id, owner_user_id)
             values ($1, $2, $3, $4, coalesce($5, '{}'::jsonb), $6::text::uuid, $7)
             on conflict (session_key, agent) do update set updated_at = now(),
               title = case when $8 then excluded.title
                            else rh_conversations.title end,
               settings = case when $9 then excluded.settings
                               else rh_conversations.settings end,
               task_id = case when $10 then excluded.task_id
                              else rh_conversations.task_id end,
               owner_user_id = coalesce(rh_conversations.owner_user_id, excluded.owner_user_id)
             returning id",
            upsert.session_key,
            upsert.agent,
            upsert.channel,
            upsert.title,
            settings,
            upsert.task_id,
            upsert.owner_user_id,
            upsert.title.is_some(),
            upsert.settings.is_some(),
            upsert.task_id.is_some(),
        )
        .fetch_one(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("conversation upsert", source))?;
        Ok(record.id)
    }
}
