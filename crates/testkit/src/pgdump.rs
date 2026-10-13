use std::sync::LazyLock;

use chrono::DateTime;
use chrono::Utc;
use regex::Regex;
use serde_json::Value;
use sqlx::PgPool;
use sqlx::Row as _;
use sqlx::Transaction;
use sqlx::postgres::Postgres;

use crate::Error;
use crate::Json;

pub(crate) const PG_TS: &str = r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(\.\d+)?\+00$";
pub(crate) const ISO_TS: &str = r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:\d{2})";

static PG_ONLY: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(PG_TS).ok());
static ISO_ONLY: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(ISO_TS).ok());
static ISO_EXACT: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(&format!("^{ISO_TS}$")).ok());
static UUID_SHAPE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}").ok()
});

const CONVERSATION_SELECT: &str = "SELECT id::text, session_key::text, agent::text, channel::text, channel_id::text, \
     bot_identity::text, title::text, settings::text, active::text, created_at::text, \
     updated_at::text, task_id::text, owner_user_id::text \
     FROM rh_conversations ORDER BY session_key, agent";

const MESSAGE_SELECT: &str = "SELECT id::text, conversation_id::text, agent::text, channel::text, role::text, \
     content::text, tool_name::text, tool_args::text, tool_result::text, metadata::text, \
     access_count::text, last_accessed_at::text, created_at::text, embed_failures::text, \
     embed_error::text, embed_status::text, owner_user_id::text, content_hash::text \
     FROM rh_messages ORDER BY ctid";

pub async fn queue_present(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let row =
        sqlx::query("select to_regclass('graphile_worker._private_jobs') is not null as present")
            .fetch_one(pool)
            .await?;
    Ok(row.try_get::<Option<bool>, _>(0)?.unwrap_or(false))
}

pub async fn capture_tx(pool: &PgPool) -> Result<Transaction<'static, Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let mode = if queue_present(pool).await? {
        "off"
    } else {
        "on"
    };
    sqlx::query("select set_config('rivet.defer_embed_enqueue', $1, true) as mode")
        .bind(mode)
        .fetch_one(&mut *tx)
        .await?;
    Ok(tx)
}

pub async fn dump_rivetos(
    pool: &PgPool,
    started: DateTime<Utc>,
    namer: &mut PgNamer,
) -> Result<Json, Error> {
    let connection = pool.acquire().await.map_err(|error| dump_error(&error))?;
    let mut guard = connection;
    sqlx::raw_sql("SET TIME ZONE 'UTC'")
        .execute(&mut *guard)
        .await
        .map_err(|error| dump_error(&error))?;
    let mut rows = serde_json::Map::new();
    rows.insert(
        "ros_conversations".to_string(),
        dump_conversations(&mut guard, started, namer).await?,
    );
    rows.insert(
        "ros_messages".to_string(),
        dump_messages(&mut guard, started, namer).await?,
    );
    rows.insert(
        "graphile_jobs".to_string(),
        dump_jobs(pool, &mut guard, started, namer).await?,
    );
    let summaries = sqlx::query("SELECT count(*)::text AS n FROM rh_summaries")
        .fetch_one(&mut *guard)
        .await
        .map_err(|error| dump_error(&error))?;
    rows.insert(
        "ros_summaries_count".to_string(),
        number_text(
            &summaries
                .try_get::<String, _>("n")
                .map_err(|error| dump_error(&error))?,
        ),
    );
    Ok(Value::Object(rows))
}

fn dump_error(source: &sqlx::Error) -> Error {
    Error::Format {
        path: std::path::PathBuf::from("pg replay"),
        message: source.to_string(),
    }
}

fn text_cell(raw: Option<String>, started: DateTime<Utc>, namer: &mut PgNamer) -> Json {
    match raw {
        Some(raw) => namer.normalize(&raw, started),
        None => Json::Null,
    }
}

fn bool_cell(raw: Option<&str>) -> Json {
    Json::Bool(raw == Some("true"))
}

fn count_cell(raw: Option<String>) -> Json {
    raw.map_or(Json::Null, |raw| number_text(&raw))
}

fn id_cell(raw: Option<String>, kind: IdKind, started: DateTime<Utc>, namer: &mut PgNamer) -> Json {
    match raw {
        Some(uuid) if looks_like_uuid(&uuid) => Json::String(match kind {
            IdKind::Conversation => namer.conv(&uuid),
            IdKind::Message => namer.msg(&uuid),
        }),
        raw => text_cell(raw, started, namer),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IdKind {
    Conversation,
    Message,
}

async fn dump_conversations(
    guard: &mut sqlx::pool::PoolConnection<Postgres>,
    started: DateTime<Utc>,
    namer: &mut PgNamer,
) -> Result<Json, Error> {
    let fetched = sqlx::query(CONVERSATION_SELECT)
        .fetch_all(&mut **guard)
        .await
        .map_err(|error| dump_error(&error))?;
    let mut out = Vec::new();
    for row in fetched {
        let column = |index: usize| -> Result<Option<String>, Error> {
            row.try_get(index).map_err(|error| dump_error(&error))
        };
        out.push(serde_json::json!({
            "id": id_cell(column(0)?, IdKind::Conversation, started, namer),
            "session_key": text_cell(column(1)?, started, namer),
            "agent": text_cell(column(2)?, started, namer),
            "channel": text_cell(column(3)?, started, namer),
            "channel_id": text_cell(column(4)?, started, namer),
            "bot_identity": text_cell(column(5)?, started, namer),
            "title": text_cell(column(6)?, started, namer),
            "settings": text_cell(column(7)?, started, namer),
            "active": bool_cell(column(8)?.as_deref()),
            "created_at": text_cell(column(9)?, started, namer),
            "updated_at": text_cell(column(10)?, started, namer),
            "task_id": text_cell(column(11)?, started, namer),
            "owner_user_id": text_cell(column(12)?, started, namer),
        }));
    }
    Ok(Json::Array(out))
}

async fn dump_messages(
    guard: &mut sqlx::pool::PoolConnection<Postgres>,
    started: DateTime<Utc>,
    namer: &mut PgNamer,
) -> Result<Json, Error> {
    let fetched = sqlx::query(MESSAGE_SELECT)
        .fetch_all(&mut **guard)
        .await
        .map_err(|error| dump_error(&error))?;
    let mut out = Vec::new();
    for row in fetched {
        let column = |index: usize| -> Result<Option<String>, Error> {
            row.try_get(index).map_err(|error| dump_error(&error))
        };
        out.push(serde_json::json!({
            "id": id_cell(column(0)?, IdKind::Message, started, namer),
            "conversation_id": id_cell(column(1)?, IdKind::Conversation, started, namer),
            "agent": text_cell(column(2)?, started, namer),
            "channel": text_cell(column(3)?, started, namer),
            "role": text_cell(column(4)?, started, namer),
            "content": text_cell(column(5)?, started, namer),
            "tool_name": text_cell(column(6)?, started, namer),
            "tool_args": text_cell(column(7)?, started, namer),
            "tool_result": text_cell(column(8)?, started, namer),
            "metadata": text_cell(column(9)?, started, namer),
            "access_count": count_cell(column(10)?),
            "last_accessed_at": text_cell(column(11)?, started, namer),
            "created_at": text_cell(column(12)?, started, namer),
            "embed_failures": count_cell(column(13)?),
            "embed_error": text_cell(column(14)?, started, namer),
            "embed_status": text_cell(column(15)?, started, namer),
            "owner_user_id": text_cell(column(16)?, started, namer),
            "content_hash": text_cell(column(17)?, started, namer),
        }));
    }
    Ok(Json::Array(out))
}

async fn dump_jobs(
    pool: &PgPool,
    guard: &mut sqlx::pool::PoolConnection<Postgres>,
    started: DateTime<Utc>,
    namer: &mut PgNamer,
) -> Result<Json, Error> {
    if !queue_present(pool)
        .await
        .map_err(|error| dump_error(&error))?
    {
        return Ok(Json::Array(Vec::new()));
    }
    let fetched = sqlx::query(
        "SELECT t.identifier AS task, q.name AS queue, j.payload::text AS payload, \
                j.key::text AS key, j.priority::text AS priority, \
                j.max_attempts::text AS max_attempts, j.attempts::text AS attempts \
           FROM graphile_worker._private_jobs j \
           JOIN graphile_worker._private_tasks t ON t.id = j.task_id \
           LEFT JOIN graphile_worker._private_job_queues q ON q.id = j.job_queue_id \
          ORDER BY j.id",
    )
    .fetch_all(&mut **guard)
    .await
    .map_err(|error| dump_error(&error))?;
    let mut jobs = Vec::new();
    for row in fetched {
        let column = |name: &'static str| -> Result<String, Error> {
            row.try_get(name).map_err(|error| dump_error(&error))
        };
        let nullable_column = |name: &'static str| -> Result<Option<String>, Error> {
            row.try_get(name).map_err(|error| dump_error(&error))
        };
        jobs.push(serde_json::json!({
            "task": column("task")?,
            "queue": nullable_column("queue")?,
            "payload": namer.rewrite_embedded(&column("payload")?, started),
            "key": nullable_column("key")?.map(|k| namer.rewrite_embedded(&k, started)),
            "priority": number_text(&column("priority")?),
            "max_attempts": number_text(&column("max_attempts")?),
            "attempts": number_text(&column("attempts")?),
        }));
    }
    Ok(Json::Array(jobs))
}

fn number_text(raw: &str) -> Json {
    raw.parse::<i64>().map_or(Json::Null, Json::from)
}

fn looks_like_uuid(text: &str) -> bool {
    UUID_SHAPE.as_ref().is_some_and(|uuid| uuid.is_match(text))
}

fn parse_loose(text: &str, started: DateTime<Utc>) -> bool {
    let parsed = if text.ends_with("+00") {
        chrono::NaiveDateTime::parse_from_str(text.trim_end_matches("+00"), "%Y-%m-%d %H:%M:%S%.f")
            .ok()
            .map(|naive| naive.and_utc())
    } else {
        DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|parsed| parsed.with_timezone(&Utc))
    };
    parsed.is_some_and(|parsed| parsed >= started)
}

fn timestamp_text(text: &str, started: DateTime<Utc>) -> bool {
    let pg = PG_ONLY
        .as_ref()
        .is_some_and(|pattern| pattern.is_match(text));
    let iso = ISO_EXACT
        .as_ref()
        .is_some_and(|pattern| pattern.is_match(text));
    (pg || iso) && parse_loose(text, started)
}

#[derive(Debug, Default)]
pub struct PgNamer {
    conv: Vec<String>,
    msg: Vec<String>,
    ids: Vec<String>,
    now: Vec<String>,
}

impl PgNamer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            conv: Vec::new(),
            msg: Vec::new(),
            ids: Vec::new(),
            now: Vec::new(),
        }
    }

    fn assign(bucket: &mut Vec<String>, text: &str, prefix: &str) -> String {
        if let Some(index) = bucket.iter().position(|seen| seen == text) {
            return format!("{prefix}-{}", index + 1);
        }
        bucket.push(text.to_string());
        format!("{prefix}-{}", bucket.len())
    }

    pub fn conv(&mut self, uuid: &str) -> String {
        Self::assign(&mut self.conv, uuid, "conv")
    }

    pub fn msg(&mut self, uuid: &str) -> String {
        Self::assign(&mut self.msg, uuid, "msg")
    }

    fn any(&mut self, uuid: &str) -> String {
        if let Some(index) = self.conv.iter().position(|seen| seen == uuid) {
            return format!("conv-{}", index + 1);
        }
        if let Some(index) = self.msg.iter().position(|seen| seen == uuid) {
            return format!("msg-{}", index + 1);
        }
        Self::assign(&mut self.ids, uuid, "id")
    }

    fn now(&mut self, text: &str) -> String {
        if let Some(index) = self.now.iter().position(|seen| seen == text) {
            return format!("<now-{}>", index + 1);
        }
        self.now.push(text.to_string());
        format!("<now-{}>", self.now.len())
    }

    pub fn normalize(&mut self, text: &str, started: DateTime<Utc>) -> Json {
        if timestamp_text(text, started) {
            return Json::String(self.now(text));
        }
        let mut replaced = text.to_string();
        if let Some(iso) = ISO_ONLY.as_ref() {
            for found in iso.find_iter(text) {
                let candidate = found.as_str().to_string();
                if parse_loose(&candidate, started) {
                    let placeholder = self.now(&candidate);
                    replaced = replaced.replace(&candidate, &placeholder);
                }
            }
        }
        Json::String(replaced)
    }

    pub fn rewrite_embedded(&mut self, text: &str, started: DateTime<Utc>) -> Json {
        let stamped = self.normalize(text, started);
        let Json::String(stamped) = stamped else {
            return stamped;
        };
        let mut out = stamped.clone();
        if let Some(uuid) = UUID_SHAPE.as_ref() {
            for found in uuid.find_iter(&stamped) {
                let candidate = found.as_str().to_string();
                let placeholder = self.any(&candidate);
                out = out.replace(&candidate, &placeholder);
            }
        }
        Json::String(out)
    }
}

pub fn slice_units(text: &str, max: usize) -> String {
    let mut units = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        let width = ch.len_utf16();
        if units + width > max {
            if width == 2 && units + 1 == max {
                out.push('\u{fffd}');
            }
            return out;
        }
        units += width;
        out.push(ch);
    }
    out
}

pub fn cap_units(text: &str, max: usize) -> String {
    let mut units = 0usize;
    for (index, ch) in text.char_indices() {
        if units + ch.len_utf16() > max {
            return text
                .get(..index)
                .map_or_else(|| text.to_string(), str::to_string);
        }
        units += ch.len_utf16();
    }
    text.to_string()
}

pub fn cap_and_mark(text: &str, max: usize) -> String {
    let capped = cap_units(text, max);
    if capped.len() == text.len() {
        capped
    } else {
        format!("{capped}\n\u{2026}[truncated]")
    }
}

pub fn canonical(value: &Json) -> String {
    match value {
        Json::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let body: Vec<String> = keys
                .iter()
                .map(|key| {
                    let leaf = map.get(*key).unwrap_or(&Json::Null);
                    format!("{key:?}:{}", canonical(leaf))
                })
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Json::Array(items) => {
            let body: Vec<String> = items.iter().map(canonical).collect();
            format!("[{}]", body.join(","))
        }
        Json::Number(number) => {
            let as_f64 = number.as_f64().unwrap_or(f64::NAN);
            format!("{as_f64:?}")
        }
        other @ (Json::Null | Json::Bool(_) | Json::String(_)) => format!("{other}"),
    }
}

pub fn js_number(value: &Json) -> Json {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the JS model stores integral doubles; the guard keeps them in i64 range"
    )]
    fn to_i64(as_f64: f64) -> i64 {
        as_f64 as i64
    }
    match value {
        Json::Number(number) => {
            let as_f64 = number.as_f64().unwrap_or(f64::NAN);
            if as_f64.is_finite()
                && as_f64.fract() == 0.0
                && as_f64.abs() < 9.007_199_254_740_992e15
            {
                Json::from(to_i64(as_f64))
            } else {
                Json::from(as_f64)
            }
        }
        Json::Array(items) => Json::Array(items.iter().map(js_number).collect()),
        Json::Object(map) => Json::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), js_number(item)))
                .collect(),
        ),
        other @ (Json::Null | Json::Bool(_) | Json::String(_)) => other.clone(),
    }
}

pub fn js_stringify(value: &Json, order: Option<&[String]>) -> String {
    match (value, order) {
        (Json::Object(map), Some(keys)) if keys.len() == map.len() => {
            let body: Vec<String> = keys
                .iter()
                .map(|key| {
                    let leaf = map.get(key).unwrap_or(&Json::Null);
                    format!("{}:{leaf}", serde_json::to_string(key).unwrap_or_default())
                })
                .collect();
            format!("{{{}}}", body.join(","))
        }
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

pub fn tool_response_orders(text: &str) -> Vec<Vec<String>> {
    let marker = "\"tool_response\"";
    let mut orders = Vec::new();
    let mut from = 0usize;
    while let Some(found) = text.get(from..).and_then(|rest| rest.find(marker)) {
        let start = from + found + marker.len();
        let Some(rest_start) = text.get(start..) else {
            break;
        };
        let rest = rest_start.trim_start();
        let rest = rest.strip_prefix(':').unwrap_or(rest).trim_start();
        if let Some(object) = rest.strip_prefix('{').and_then(balanced_object_text) {
            orders.push(object_key_order(object));
        }
        from = start + (rest_start.len() - rest.len());
    }
    orders
}

fn balanced_object_text(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut depth = 1usize;
    let mut in_string = false;
    let mut index = 0usize;
    while let Some(&byte) = bytes.get(index) {
        if in_string {
            match byte {
                b'\\' => index += 1,
                b'"' => in_string = false,
                _ => {}
            }
        } else {
            match byte {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return text.get(..=index);
                    }
                }
                _ => {}
            }
        }
        index += 1;
    }
    None
}

fn object_key_order(raw: &str) -> Vec<String> {
    let bytes = raw.as_bytes();
    let mut keys = Vec::new();
    let mut index = 1usize;
    while let Some(&byte) = bytes.get(index) {
        match byte {
            b'}' => break,
            b'"' => {
                let mut key = String::new();
                index += 1;
                while let Some(&inner) = bytes.get(index) {
                    if inner == b'"' {
                        break;
                    }
                    if inner == b'\\' {
                        index += 1;
                        if let Some(&escaped) = bytes.get(index) {
                            key.push(escaped as char);
                        }
                    } else {
                        key.push(inner as char);
                    }
                    index += 1;
                }
                index += 1;
                while bytes.get(index).is_some_and(|&inner| inner != b':') {
                    index += 1;
                }
                index += 1;
                while let Some(&inner) = bytes.get(index) {
                    if inner == b',' || inner == b'}' {
                        break;
                    }
                    if inner == b'"' {
                        index += 1;
                        while let Some(&escaped) = bytes.get(index) {
                            if escaped == b'"' {
                                break;
                            }
                            if escaped == b'\\' {
                                index += 1;
                            }
                            index += 1;
                        }
                    }
                    index += 1;
                }
                keys.push(key);
            }
            _ => index += 1,
        }
    }
    keys
}
