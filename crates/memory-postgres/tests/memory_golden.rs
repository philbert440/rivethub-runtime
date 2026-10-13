use memory_postgres::PostgresMemory;
use serde_json::Value;
use sqlx::PgPool;
use sqlx::Row as _;
use std::collections::BTreeMap;
use testkit::queue_present;
use wire::capture::CaptureBatchRequest;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn store(pool: &PgPool) -> PostgresMemory {
    PostgresMemory::new(pool.clone())
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value, String> {
    value
        .get(key)
        .ok_or_else(|| format!("the comparison side carries no {key} member"))
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    field(value, key)?
        .as_str()
        .ok_or_else(|| format!("the {key} member is not a string"))
}

fn str_member<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("the {key} member is not a string"))
}

fn optional_text(value: &Value, key: &str) -> Result<Option<String>, String> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(found) => found
            .as_str()
            .map(str::to_string)
            .map(Some)
            .ok_or_else(|| format!("the {key} member is not a string")),
    }
}

#[sqlx::test]
async fn capture_batch_golden(pool: PgPool) -> TestResult {
    let memory = store(&pool);
    let queue = queue_present(&pool).await?;
    let text_body = std::fs::read_to_string(testkit::recording("memory/capture-batch.json"))?;
    let golden: Value = serde_json::from_str(&text_body)?;

    let mut conv_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut uuid_to_label: BTreeMap<String, String> = BTreeMap::new();
    let recorded_results = golden
        .get("results")
        .and_then(Value::as_array)
        .ok_or("golden results are a list")?;
    for entry in recorded_results {
        let batch: CaptureBatchRequest = serde_json::from_value(field(entry, "batch")?.clone())?;
        let opts = field(entry, "opts")?;
        let owner_user_id = optional_text(opts, "ownerUserId")?;
        let allow_filesystem = match opts.get("allowFilesystem") {
            None | Some(Value::Null) => false,
            Some(found) => found
                .as_bool()
                .ok_or("the allowFilesystem member is not a boolean")?,
        };
        let options = memory_postgres::CaptureOptions {
            owner_user_id,
            allow_filesystem,
        };
        let result = memory.capture_batch(&batch, &options).await?;
        let recorded_id = text(field(entry, "result")?, "conversation_id")?.to_string();
        conv_ids
            .entry(recorded_id.clone())
            .or_insert_with(|| result.conversation_id.clone());
        uuid_to_label
            .entry(result.conversation_id.clone())
            .or_insert_with(|| recorded_id.clone());
        let actual = serde_json::to_value(&result)?;
        let mut expected = field(entry, "result")?.clone();
        if let Some(object) = expected.as_object_mut() {
            object.insert(
                "conversation_id".to_string(),
                Value::String(
                    conv_ids
                        .get(&recorded_id)
                        .cloned()
                        .ok_or("a mapped conversation id")?,
                ),
            );
        }
        assert_eq!(
            actual,
            expected,
            "capture result drifted for {:?}",
            entry.get("label")
        );
    }

    let conversations = dump_conversations(&pool).await?;
    let recorded_conversations = golden
        .get("conversations")
        .and_then(Value::as_array)
        .ok_or("golden conversations are a list")?;
    assert_eq!(conversations.len(), recorded_conversations.len());
    for (actual, expected) in conversations.iter().zip(recorded_conversations) {
        for key in [
            "session_key",
            "agent",
            "channel",
            "title",
            "task_id",
            "active",
            "owner_user_id",
        ] {
            assert_eq!(
                field(actual, key)?,
                field(expected, key)?,
                "column {key} drifted"
            );
        }
        assert_eq!(
            field(actual, "settings")?,
            field(expected, "settings")?,
            "settings drifted"
        );
    }

    let all_messages = dump_messages(&pool, &uuid_to_label).await?;
    let mut actual_messages: BTreeMap<(String, String), Value> = BTreeMap::new();
    for message in &all_messages {
        let event_id = str_member(field(message, "metadata")?, "event_id")?.to_string();
        let conversation = text(message, "conversation_id")?.to_string();
        actual_messages.insert((conversation, event_id), message.clone());
    }
    let recorded_messages = golden
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("golden messages are a list")?;
    for expected in recorded_messages {
        let key = (
            text(expected, "conversation_id")?.to_string(),
            text(field(expected, "metadata")?, "event_id")?.to_string(),
        );
        let actual = actual_messages
            .get(&key)
            .ok_or_else(|| format!("missing message {key:?}"))?;
        for key in [
            "agent",
            "channel",
            "role",
            "tool_name",
            "tool_result",
            "owner_user_id",
        ] {
            assert_field(
                field(actual, key)?,
                field(expected, key)?,
                &format!("message column {key}"),
            );
        }
        assert_eq!(
            field(actual, "content_len")?,
            field(expected, "content_len")?
        );
        assert_eq!(field(actual, "tool_args")?, field(expected, "tool_args")?);
        assert_eq!(field(actual, "metadata")?, field(expected, "metadata")?);
        assert_field(
            field(actual, "content")?,
            field(expected, "content")?,
            "message content",
        );
    }

    if queue {
        let jobs = dump_jobs(&pool).await?;
        let recorded_jobs = golden
            .get("jobs")
            .and_then(Value::as_array)
            .ok_or("golden jobs are a list")?;
        assert_eq!(jobs.len(), recorded_jobs.len());
        for (index, (actual, expected)) in jobs.iter().zip(recorded_jobs).enumerate() {
            assert_eq!(
                field(actual, "task")?,
                field(expected, "task")?,
                "job {index} task drifted"
            );
            let message = all_messages
                .get(index)
                .ok_or("a message for the recorded job")?;
            let actual_target = str_member(field(actual, "payload")?, "targetId")?;
            assert_eq!(
                field(field(expected, "payload")?, "targetTable")?,
                field(field(actual, "payload")?, "targetTable")?,
                "job {index} targetTable drifted"
            );
            assert!(
                text(message, "id")? == actual_target,
                "job {index} targets {actual_target}"
            );
            assert_eq!(
                field(actual, "max_attempts")?,
                field(expected, "max_attempts")?
            );
            assert_eq!(field(actual, "priority")?, field(expected, "priority")?);
            let actual_key = text(actual, "key")?;
            let expected_suffix = format!("-{}", text(message, "id")?);
            assert!(
                actual_key.starts_with("embed-ros_messages-")
                    && actual_key.ends_with(&expected_suffix),
                "job key {actual_key} does not embed the targeted message id"
            );
        }
    }

    let tags = dump_tags(&pool).await?;
    let recorded_tags = golden
        .get("tags")
        .and_then(Value::as_array)
        .ok_or("golden tags are a list")?;
    assert_eq!(tags.len(), recorded_tags.len());
    let by_value = |entries: &[Value]| {
        entries
            .iter()
            .map(|entry| Ok((text(entry, "value")?.to_string(), entry.clone())))
            .collect::<Result<BTreeMap<_, _>, String>>()
    };
    assert_eq!(by_value(&tags)?, by_value(recorded_tags)?);

    let finalized = sqlx::query(
        "SELECT created_at::text AS created, updated_at::text AS updated FROM rh_conversations
          WHERE active = false",
    )
    .fetch_one(&pool)
    .await?;
    let created: String = finalized.try_get("created").unwrap();
    let updated: String = finalized.try_get("updated").unwrap();
    assert!(
        updated >= created,
        "finalize must move updated_at forward: {updated} vs {created}"
    );
    Ok(())
}

fn assert_field(actual: &Value, expected: &Value, label: &str) {
    if let (Value::String(actual_text), Value::String(expected_text)) = (actual, expected)
        && let Some((prefix, remaining)) = expected_text.split_once('\u{2026}')
        && let Some(count) = remaining
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|digits| digits.parse::<usize>().ok())
    {
        assert!(
            actual_text.starts_with(prefix) && actual_text.chars().count() == count,
            "{label} drifted: abbreviated expected {expected_text:?}, actual len {}",
            actual_text.chars().count()
        );
        return;
    }
    assert_eq!(actual, expected, "{label} drifted");
}

fn decode(raw: &str) -> Result<Value, sqlx::Error> {
    serde_json::from_str(raw).map_err(|source| sqlx::Error::ColumnDecode {
        index: "json".to_string(),
        source: Box::new(source),
    })
}

async fn dump_conversations(pool: &PgPool) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    #[derive(serde::Serialize)]
    struct GoldenConversation {
        session_key: String,
        agent: String,
        channel: String,
        title: Option<String>,
        settings: Value,
        task_id: Option<String>,
        active: bool,
        owner_user_id: Option<String>,
    }
    let rows = sqlx::query(
        "SELECT session_key::text AS session_key, agent::text AS agent, channel::text AS channel,
                title::text AS title, settings::text AS settings, task_id::text AS task_id,
                active::text AS active, owner_user_id::text AS owner_user_id
           FROM rh_conversations ORDER BY session_key, agent",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        let settings: String = row.try_get("settings")?;
        out.push(serde_json::to_value(GoldenConversation {
            session_key: row.try_get("session_key")?,
            agent: row.try_get("agent")?,
            channel: row.try_get("channel")?,
            title: row.try_get("title")?,
            settings: decode(&settings)?,
            task_id: row.try_get("task_id")?,
            active: row.try_get::<String, _>("active")? == "true",
            owner_user_id: row.try_get("owner_user_id")?,
        })?);
    }
    Ok(out)
}

async fn dump_messages(
    pool: &PgPool,
    uuid_to_label: &BTreeMap<String, String>,
) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    #[derive(serde::Serialize)]
    struct GoldenMessage {
        id: String,
        conversation_id: String,
        agent: String,
        channel: String,
        role: String,
        content: String,
        tool_name: Option<String>,
        tool_args: Option<Value>,
        tool_result: Option<String>,
        metadata: Value,
        owner_user_id: Option<String>,
        content_len: i32,
    }
    let rows = sqlx::query(
        "SELECT m.id::text AS id, m.conversation_id::text AS conversation_id,
                m.agent::text AS agent,
                m.channel::text AS channel, m.role::text AS role, m.content::text AS content,
                m.tool_name::text AS tool_name, m.tool_args::text AS tool_args,
                m.tool_result::text AS tool_result, m.metadata::text AS metadata,
                m.owner_user_id::text AS owner_user_id, length(m.content) AS content_len
           FROM rh_messages m
          ORDER BY m.created_at, m.id",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        let metadata: String = row.try_get("metadata")?;
        let conversation_id: String = row.try_get("conversation_id")?;
        let tool_args: Option<String> = row.try_get("tool_args")?;
        out.push(serde_json::to_value(GoldenMessage {
            id: row.try_get("id")?,
            conversation_id: uuid_to_label
                .get(&conversation_id)
                .cloned()
                .unwrap_or(conversation_id),
            agent: row.try_get("agent")?,
            channel: row.try_get("channel")?,
            role: row.try_get("role")?,
            content: row.try_get("content")?,
            tool_name: row.try_get("tool_name")?,
            tool_args: tool_args.map(|text| decode(&text)).transpose()?,
            tool_result: row.try_get("tool_result")?,
            metadata: decode(&metadata)?,
            owner_user_id: row.try_get("owner_user_id")?,
            content_len: row.try_get("content_len")?,
        })?);
    }
    Ok(out)
}

async fn dump_jobs(pool: &PgPool) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    #[derive(serde::Serialize)]
    struct GoldenJob {
        task: String,
        payload: Value,
        key: String,
        max_attempts: i64,
        priority: i64,
    }
    let rows = sqlx::query(
        "SELECT t.identifier AS task, j.payload::text AS payload, j.key::text AS key,
                j.max_attempts::text AS max_attempts, j.priority::text AS priority
           FROM graphile_worker._private_jobs j
           JOIN graphile_worker._private_tasks t ON t.id = j.task_id
          ORDER BY j.id",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        let payload: String = row.try_get("payload")?;
        let max_attempts: String = row.try_get("max_attempts")?;
        let priority: String = row.try_get("priority")?;
        out.push(serde_json::to_value(GoldenJob {
            task: row.try_get("task")?,
            payload: decode(&payload)?,
            key: row.try_get("key")?,
            max_attempts: int(&max_attempts)?,
            priority: int(&priority)?,
        })?);
    }
    Ok(out)
}

fn int(raw: &str) -> Result<i64, sqlx::Error> {
    raw.parse::<i64>()
        .map_err(|source| sqlx::Error::ColumnDecode {
            index: raw.to_string(),
            source: Box::new(source),
        })
}

async fn dump_tags(pool: &PgPool) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    #[derive(serde::Serialize)]
    struct GoldenTag {
        entity_type: String,
        key: String,
        value: String,
        state: String,
        source: String,
    }
    let rows = sqlx::query(
        "SELECT entity_type::text AS entity_type, key::text AS key, value::text AS value,
                state::text AS state, source::text AS source
           FROM rh_tags ORDER BY value",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        out.push(serde_json::to_value(GoldenTag {
            entity_type: row.try_get("entity_type")?,
            key: row.try_get("key")?,
            value: row.try_get("value")?,
            state: row.try_get("state")?,
            source: row.try_get("source")?,
        })?);
    }
    Ok(out)
}
