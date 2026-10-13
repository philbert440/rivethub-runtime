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

fn at<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

#[sqlx::test]
async fn capture_batch_golden(pool: PgPool) -> TestResult {
    let memory = store(&pool);
    let queue = queue_present(&pool).await?;
    let text = std::fs::read_to_string(testkit::recording("memory/capture-batch.json"))?;
    let golden: Value = serde_json::from_str(&text)?;

    let mut conv_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut uuid_to_label: BTreeMap<String, String> = BTreeMap::new();
    let recorded_results = golden
        .get("results")
        .and_then(Value::as_array)
        .ok_or("golden results are a list")?;
    for entry in recorded_results {
        let batch: CaptureBatchRequest = serde_json::from_value(at(entry, "batch").clone())?;
        let options = memory_postgres::CaptureOptions {
            owner_user_id: at(at(entry, "opts"), "ownerUserId")
                .as_str()
                .map(str::to_string),
            allow_filesystem: *at(at(entry, "opts"), "allowFilesystem") == Value::Bool(true),
        };
        let result = memory.capture_batch(&batch, &options).await?;
        let recorded_id = at(at(entry, "result"), "conversation_id")
            .as_str()
            .ok_or("a recorded conversation id")?
            .to_string();
        conv_ids
            .entry(recorded_id.clone())
            .or_insert_with(|| result.conversation_id.clone());
        uuid_to_label
            .entry(result.conversation_id.clone())
            .or_insert_with(|| recorded_id.clone());
        let actual = serde_json::json!({
            "ok": result.ok,
            "conversation_id": result.conversation_id,
            "inserted": result.inserted,
            "skipped": result.skipped,
        });
        let mut expected = at(entry, "result").clone();
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
            assert_eq!(at(actual, key), at(expected, key), "column {key} drifted");
        }
        assert_eq!(
            at(actual, "settings"),
            at(expected, "settings"),
            "settings drifted"
        );
    }

    let all_messages = dump_messages(&pool, &uuid_to_label).await?;
    let mut actual_messages: BTreeMap<(String, String), Value> = BTreeMap::new();
    for message in &all_messages {
        let event_id = at(at(message, "metadata"), "event_id")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let conversation = at(message, "conversation_id")
            .as_str()
            .unwrap_or_default()
            .to_string();
        actual_messages.insert((conversation, event_id), message.clone());
    }
    let recorded_messages = golden
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("golden messages are a list")?;
    for expected in recorded_messages {
        let key = (
            at(expected, "conversation_id")
                .as_str()
                .unwrap_or_default()
                .to_string(),
            at(at(expected, "metadata"), "event_id")
                .as_str()
                .unwrap_or_default()
                .to_string(),
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
                at(actual, key),
                at(expected, key),
                &format!("message column {key}"),
            );
        }
        assert_eq!(at(actual, "content_len"), at(expected, "content_len"));
        assert_eq!(at(actual, "tool_args"), at(expected, "tool_args"));
        assert_eq!(at(actual, "metadata"), at(expected, "metadata"));
        assert_field(
            at(actual, "content"),
            at(expected, "content"),
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
                at(actual, "task"),
                at(expected, "task"),
                "job {index} task drifted"
            );
            let message = all_messages
                .get(index)
                .ok_or("a message for the recorded job")?;
            let actual_target = at(at(actual, "payload"), "targetId")
                .as_str()
                .unwrap_or_default();
            assert_eq!(
                at(at(expected, "payload"), "targetTable"),
                at(at(actual, "payload"), "targetTable"),
                "job {index} targetTable drifted"
            );
            assert!(
                at(message, "id").as_str() == Some(actual_target),
                "job {index} targets {actual_target}"
            );
            assert_eq!(at(actual, "max_attempts"), at(expected, "max_attempts"));
            assert_eq!(at(actual, "priority"), at(expected, "priority"));
            let actual_key = at(actual, "key").as_str().unwrap_or_default();
            let expected_suffix = format!("-{}", at(message, "id").as_str().unwrap_or_default());
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
            .map(|entry| {
                (
                    at(entry, "value").as_str().unwrap_or_default().to_string(),
                    entry.clone(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(by_value(&tags), by_value(recorded_tags));

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

async fn dump_conversations(pool: &PgPool) -> Result<Vec<Value>, sqlx::Error> {
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
        out.push(serde_json::json!({
            "session_key": row.try_get::<String, _>("session_key")?,
            "agent": row.try_get::<String, _>("agent")?,
            "channel": row.try_get::<String, _>("channel")?,
            "title": row.try_get::<Option<String>, _>("title")?,
            "settings": serde_json::from_str::<Value>(&settings).unwrap_or_default(),
            "task_id": row.try_get::<Option<String>, _>("task_id")?,
            "active": row.try_get::<String, _>("active")? == "true",
            "owner_user_id": row.try_get::<Option<String>, _>("owner_user_id")?,
        }));
    }
    Ok(out)
}

async fn dump_messages(
    pool: &PgPool,
    uuid_to_label: &BTreeMap<String, String>,
) -> Result<Vec<Value>, sqlx::Error> {
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
        out.push(serde_json::json!({
            "id": row.try_get::<String, _>("id")?,
            "conversation_id": uuid_to_label.get(&conversation_id).cloned().unwrap_or(conversation_id),
            "agent": row.try_get::<String, _>("agent")?,
            "channel": row.try_get::<String, _>("channel")?,
            "role": row.try_get::<String, _>("role")?,
            "content": row.try_get::<String, _>("content")?,
            "tool_name": row.try_get::<Option<String>, _>("tool_name")?,
            "tool_args": row
                .try_get::<Option<String>, _>("tool_args")?
                .and_then(|text| serde_json::from_str::<Value>(&text).ok()),
            "tool_result": row.try_get::<Option<String>, _>("tool_result")?,
            "metadata": serde_json::from_str::<Value>(&metadata).unwrap_or_default(),
            "owner_user_id": row.try_get::<Option<String>, _>("owner_user_id")?,
            "content_len": row.try_get::<i32, _>("content_len")?,
        }));
    }
    Ok(out)
}

async fn dump_jobs(pool: &PgPool) -> Result<Vec<Value>, sqlx::Error> {
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
        out.push(serde_json::json!({
            "task": row.try_get::<String, _>("task")?,
            "payload": serde_json::from_str::<Value>(&payload).unwrap_or_default(),
            "key": row.try_get::<String, _>("key")?,
            "max_attempts": max_attempts.parse::<i64>().unwrap_or_default(),
            "priority": priority.parse::<i64>().unwrap_or_default(),
        }));
    }
    Ok(out)
}

async fn dump_tags(pool: &PgPool) -> Result<Vec<Value>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT entity_type::text AS entity_type, key::text AS key, value::text AS value,
                state::text AS state, source::text AS source
           FROM rh_tags ORDER BY value",
    )
    .fetch_all(pool)
    .await?;
    let mut out = Vec::new();
    for row in rows {
        out.push(serde_json::json!({
            "entity_type": row.try_get::<String, _>("entity_type")?,
            "key": row.try_get::<String, _>("key")?,
            "value": row.try_get::<String, _>("value")?,
            "state": row.try_get::<String, _>("state")?,
            "source": row.try_get::<String, _>("source")?,
        }));
    }
    Ok(out)
}
