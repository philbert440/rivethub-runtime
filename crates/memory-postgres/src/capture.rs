use crate::error::Error;
use crate::project;
use chrono::DateTime;
use chrono::Utc;
use sqlx::Acquire as _;
use sqlx::PgPool;
use sqlx::Transaction;
use sqlx::postgres::Postgres;
use uuid::Uuid;
use wire::JsonObject;
use wire::capture::CaptureMessage;
use wire::message::Role;

pub const MAX_CONTENT_UNITS: usize = 16_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CaptureOptions {
    pub owner_user_id: Option<String>,
    pub allow_filesystem: bool,
}

#[derive(Debug)]
pub struct CaptureWriter<'a> {
    pub(crate) tx: Transaction<'a, Postgres>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConversationUpsert<'a> {
    pub session_key: &'a str,
    pub agent: &'a str,
    pub channel: Option<&'a str>,
    pub title: Option<&'a str>,
    pub settings: Option<&'a JsonObject>,
    pub task_id: Option<&'a str>,
    pub owner_user_id: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationEnsure<'a> {
    pub session_key: &'a str,
    pub agent: &'a str,
    pub channel: Option<&'a str>,
    pub title: Option<&'a str>,
    pub settings: &'a JsonObject,
    pub task_id: Option<&'a str>,
    pub owner_user_id: Option<&'a str>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRecord {
    pub id: Uuid,
    pub created: bool,
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationUpdate<'a> {
    pub id: Uuid,
    pub updated_at: Option<DateTime<Utc>>,
    pub active: bool,
    pub title: Option<&'a str>,
    pub settings: &'a JsonObject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow<'a> {
    pub conversation_id: Uuid,
    pub agent: &'a str,
    pub channel: &'a str,
    pub role: Role,
    pub content: &'a str,
    pub tool_name: Option<&'a str>,
    pub tool_args: Option<&'a JsonObject>,
    pub tool_result: Option<&'a str>,
    pub metadata: &'a JsonObject,
    pub owner_user_id: Option<&'a str>,
    pub created_at: Option<DateTime<Utc>>,
}

impl<'a> CaptureWriter<'a> {
    pub async fn begin(pool: &'a PgPool, session_key: &str) -> Result<Self, Error> {
        let mut tx = pool
            .begin()
            .await
            .map_err(|source| Error::operation("capture begin", source))?;
        let _locked = sqlx::query!(
            "select pg_advisory_xact_lock(hashtext($1)) is not null as locked",
            session_key
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|source| Error::operation("capture lock", source))?;
        let _ = sqlx::query_scalar!(
            "select set_config('rivet.defer_embed_enqueue',
                     case when to_regclass('graphile_worker._private_jobs') is null
                          then 'on' else 'off' end, true) as mode"
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|source| Error::operation("embed defer", source))?;
        Ok(Self { tx })
    }

    pub async fn commit(self) -> Result<(), Error> {
        self.tx
            .commit()
            .await
            .map_err(|source| Error::operation("capture commit", source))
    }

    pub async fn adopt_owner(&mut self, id: Uuid, owner: &str) -> Result<(), Error> {
        sqlx::query!(
            "update rh_conversations set owner_user_id = $2
              where id = $1 and owner_user_id is null",
            id,
            owner
        )
        .execute(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("owner adoption", source))?;
        Ok(())
    }

    pub async fn backfill_task_id(&mut self, id: Uuid, task_id: &str) -> Result<(), Error> {
        sqlx::query!(
            "update rh_conversations set task_id = $2::text::uuid
              where id = $1 and task_id is null",
            id,
            task_id
        )
        .execute(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("task backfill", source))?;
        Ok(())
    }

    pub async fn update_conversation(
        &mut self,
        update: &ConversationUpdate<'_>,
    ) -> Result<(), Error> {
        let settings = serde_json::to_value(update.settings)
            .map_err(|source| Error::protocol(source.to_string()))?;
        sqlx::query!(
            "update rh_conversations
                set updated_at = coalesce($2, now()),
                    active = $3,
                    title = coalesce($4, title),
                    settings = $5
              where id = $1",
            update.id,
            update.updated_at,
            update.active,
            update.title,
            settings
        )
        .execute(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("conversation update", source))?;
        Ok(())
    }

    pub async fn touch_settings(&mut self, id: Uuid, settings: &JsonObject) -> Result<(), Error> {
        let json =
            serde_json::to_value(settings).map_err(|source| Error::protocol(source.to_string()))?;
        sqlx::query!(
            "update rh_conversations set updated_at = now(), settings = $2 where id = $1",
            id,
            json
        )
        .execute(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("conversation settings", source))?;
        Ok(())
    }

    pub async fn finalize_conversation(&mut self, id: Uuid) -> Result<(), Error> {
        sqlx::query!(
            "update rh_conversations set active = false, updated_at = now()
              where id = $1 and active = true",
            id
        )
        .execute(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("conversation finalize", source))?;
        Ok(())
    }

    pub async fn insert_message(&mut self, row: &MessageRow<'_>) -> Result<Uuid, Error> {
        let tool_args = row
            .tool_args
            .map(serde_json::to_value)
            .transpose()
            .map_err(|source| Error::protocol(source.to_string()))?;
        let metadata = serde_json::to_value(row.metadata)
            .map_err(|source| Error::protocol(source.to_string()))?;
        let record = sqlx::query!(
            "insert into rh_messages
               (conversation_id, agent, channel, role, content, tool_name, tool_args,
                tool_result, metadata, owner_user_id, created_at)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, coalesce($11, now()))
             returning id",
            row.conversation_id,
            row.agent,
            row.channel,
            row.role.to_string(),
            row.content,
            row.tool_name,
            tool_args,
            row.tool_result,
            metadata,
            row.owner_user_id,
            row.created_at,
        )
        .fetch_one(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("message insert", source))?;
        Ok(record.id)
    }

    pub async fn seen_event_ids(
        &mut self,
        conversation_id: Uuid,
        event_ids: &[String],
    ) -> Result<Vec<String>, Error> {
        if event_ids.is_empty() {
            return Ok(Vec::new());
        }
        let seen = sqlx::query_scalar!(
            "select metadata->>'event_id' from rh_messages
              where conversation_id = $1 and metadata->>'event_id' = any($2)",
            conversation_id,
            event_ids
        )
        .fetch_all(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("event id lookup", source))?;
        Ok(seen.into_iter().flatten().collect())
    }

    pub async fn stored_assistant_uuids(
        &mut self,
        conversation_id: Uuid,
    ) -> Result<Vec<String>, Error> {
        let stored = sqlx::query_scalar!(
            "select metadata->>'uuid' from rh_messages
              where conversation_id = $1 and role = 'assistant' and jsonb_exists(metadata, 'uuid')",
            conversation_id
        )
        .fetch_all(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("uuid lookup", source))?;
        Ok(stored.into_iter().flatten().collect())
    }

    pub async fn stored_tool_calls(
        &mut self,
        conversation_id: Uuid,
    ) -> Result<Vec<(Option<String>, Option<String>)>, Error> {
        let rows = sqlx::query!(
            "select tool_name, tool_args::text as args from rh_messages
              where conversation_id = $1 and role = 'tool'",
            conversation_id
        )
        .fetch_all(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("tool call lookup", source))?;
        Ok(rows
            .into_iter()
            .map(|row| (row.tool_name, row.args))
            .collect())
    }

    pub async fn stored_user_contents(
        &mut self,
        conversation_id: Uuid,
    ) -> Result<Vec<String>, Error> {
        sqlx::query_scalar!(
            "select content from rh_messages where conversation_id = $1 and role = 'user'",
            conversation_id
        )
        .fetch_all(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("user content lookup", source))
    }

    pub async fn ingest_key_seen(
        &mut self,
        conversation_id: Uuid,
        key: &str,
    ) -> Result<bool, Error> {
        let record = sqlx::query!(
            "select exists(select 1 from rh_messages
                where conversation_id = $1 and metadata->>'ingest_key' = $2) as seen",
            conversation_id,
            key
        )
        .fetch_one(&mut *self.tx)
        .await
        .map_err(|source| Error::operation("ingest key lookup", source))?;
        Ok(record.seen.unwrap_or(false))
    }

    pub async fn apply_project_tag(
        &mut self,
        conversation_id: Uuid,
        tag: &project::ProjectTag,
    ) -> Result<(), Error> {
        let mut savepoint = self
            .tx
            .begin()
            .await
            .map_err(|source| Error::operation("tag savepoint", source))?;
        sqlx::query!(
            "insert into rh_tags (entity_type, entity_id, key, value, display, source, state)
             values ('conversation', $1, $2, $3, $4, 'rule', 'accepted')
             on conflict (entity_type, entity_id, key, value) do nothing",
            conversation_id,
            tag.key.as_str(),
            tag.value.as_str(),
            tag.display.as_str()
        )
        .execute(&mut *savepoint)
        .await
        .map_err(|source| Error::operation("project tag", source))?;
        savepoint
            .commit()
            .await
            .map_err(|source| Error::operation("project tag", source))
    }
}

pub(crate) fn utf16_len(text: &str) -> u64 {
    text.chars().map(char::len_utf16).sum::<usize>() as u64
}

pub(crate) fn cap_units(text: &str, max: usize) -> (&str, bool) {
    let mut units = 0usize;
    for (idx, ch) in text.char_indices() {
        if units + ch.len_utf16() > max {
            return (text.get(..idx).unwrap_or_default(), true);
        }
        units += ch.len_utf16();
    }
    (text, false)
}

pub(crate) struct PreparedMessage {
    pub(crate) content: String,
    pub(crate) tool_result: Option<String>,
    pub(crate) metadata: JsonObject,
    pub(crate) created_at: Option<DateTime<Utc>>,
}

pub(crate) fn prepare_capture_message(message: &CaptureMessage) -> Result<PreparedMessage, Error> {
    let (content, content_truncated) = cap_units(&message.content, MAX_CONTENT_UNITS);
    let full_content = utf16_len(&message.content);
    let full_tool_result = message.tool_result.as_deref().map_or(0, utf16_len);
    let (tool_result, tool_truncated) = match message.tool_result.as_deref() {
        Some(text) => {
            let (capped, truncated) = cap_units(text, MAX_CONTENT_UNITS);
            (Some(capped.to_string()), truncated)
        }
        None => (None, false),
    };
    let metadata: JsonObject = message.metadata.clone().unwrap_or_default();
    let mut metadata =
        serde_json::to_value(metadata).map_err(|source| Error::protocol(source.to_string()))?;
    if !metadata.is_object() {
        metadata = serde_json::json!({});
    }
    if let Some(object) = metadata.as_object_mut() {
        object.insert("event_id".to_string(), serde_json::json!(message.event_id));
        if content_truncated {
            object.insert("truncated".to_string(), serde_json::json!(true));
            object.insert(
                "full_content_length".to_string(),
                serde_json::json!(full_content),
            );
        }
        if tool_truncated {
            object.insert(
                "full_tool_result_length".to_string(),
                serde_json::json!(full_tool_result),
            );
        }
    }
    let metadata: JsonObject =
        serde_json::from_value(metadata).map_err(|source| Error::protocol(source.to_string()))?;
    Ok(PreparedMessage {
        content: content.to_string(),
        tool_result,
        metadata,
        created_at: message.created_at.as_deref().and_then(parse_timestamp),
    })
}

pub(crate) fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}
