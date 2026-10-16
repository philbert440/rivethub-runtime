use chrono::DateTime;
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

use crate::SqliteError;
use crate::SqliteStore;
use crate::time::stamp;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationDirectory {
    pub id: String,
    pub cwd: String,
}

#[derive(Debug, Deserialize)]
struct SettingsDocument {
    cwd: Option<String>,
}

impl SqliteStore {
    pub async fn conversation_directories(
        &self,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<ConversationDirectory>, SqliteError> {
        self.with_conn(move |conn| directories(conn, since)).await
    }

    pub async fn apply_project_rule(
        &self,
        conversation_id: String,
        value: String,
        display: String,
    ) -> Result<u64, SqliteError> {
        let now = stamp(self.now());
        self.with_conn(move |conn| apply(conn, &conversation_id, &value, &display, &now))
            .await
    }
}

fn directories(
    conn: &rusqlite::Connection,
    since: Option<DateTime<Utc>>,
) -> Result<Vec<ConversationDirectory>, SqliteError> {
    let sql = match since {
        Some(_) => {
            "SELECT id, settings FROM ros_conversations \
             WHERE created_at >= :since OR updated_at >= :since"
        }
        None => "SELECT id, settings FROM ros_conversations",
    };
    let mut statement = conn
        .prepare(sql)
        .map_err(SqliteError::query("project directories"))?;
    let mark = since.map(stamp);
    let mut rows = match &mark {
        Some(mark) => statement
            .query(rusqlite::named_params! { ":since": mark })
            .map_err(SqliteError::query("project directories"))?,
        None => statement
            .query([])
            .map_err(SqliteError::query("project directories"))?,
    };
    let mut found = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(SqliteError::query("project directories"))?
    {
        let id: String = row
            .get(0)
            .map_err(SqliteError::query("project directories"))?;
        let settings: String = row
            .get(1)
            .map_err(SqliteError::query("project directories"))?;
        if let Some(cwd) = cwd(&settings)? {
            found.push(ConversationDirectory { id, cwd });
        }
    }
    Ok(found)
}

fn cwd(settings: &str) -> Result<Option<String>, SqliteError> {
    let parsed: SettingsDocument =
        serde_json::from_str(settings).map_err(|source| SqliteError::StoredJson {
            column: "settings",
            source,
        })?;
    match parsed.cwd {
        Some(value) if !value.trim().is_empty() => Ok(Some(value)),
        Some(_) | None => Ok(None),
    }
}

fn apply(
    conn: &rusqlite::Connection,
    conversation_id: &str,
    value: &str,
    display: &str,
    now: &str,
) -> Result<u64, SqliteError> {
    let changed = conn
        .execute(
            "INSERT OR IGNORE INTO ros_tags (
                id, entity_type, entity_id, key, value, display, source, state, created_at, updated_at
             ) VALUES (?1, 'conversation', ?2, 'project', ?3, ?4, 'rule', 'accepted', ?5, ?5)",
            (Uuid::new_v4().to_string(), conversation_id, value, display, now),
        )
        .map_err(SqliteError::query("project rule"))?;
    u64::try_from(changed).map_err(SqliteError::Count)
}
