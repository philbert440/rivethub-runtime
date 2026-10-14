use serde_json::Value;
use sqlx::PgPool;
use testkit::Json;
use testkit::capture_pg;
use testkit::capture_pg::PgOutcome;
use testkit::pgdump::dump_rivetos;
use testkit::pgdump::queue_present;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDEN_FILES: [&str; 5] = [
    "transcript.json",
    "task.json",
    "routed.json",
    "hook.json",
    "errors.json",
];

fn at<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

async fn reset(pool: &PgPool) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::query("truncate rh_conversations cascade")
        .execute(pool)
        .await?;
    sqlx::query("delete from rh_tags").execute(pool).await?;
    sqlx::query("delete from rh_tag_taxonomy")
        .execute(pool)
        .await?;
    if queue_present(pool).await? {
        sqlx::query("delete from graphile_worker._private_jobs")
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn replay_public_arm(pool: PgPool) -> TestResult {
    let queue = queue_present(&pool).await?;
    for file in GOLDEN_FILES {
        let path = testkit::recording(&format!("capture/pg/{file}"));
        for scenario in capture_pg::scenarios(&path)? {
            if scenario.schema != "public" {
                continue;
            }
            let mut scenario = scenario.clone();
            if !queue {
                drop_graphile_rows(&mut scenario);
            }
            reset(&pool).await?;
            let started = chrono::Utc::now() - chrono::Duration::seconds(5);
            let mut outcomes: Vec<PgOutcome> = Vec::new();
            let mut namer = testkit::PgNamer::new();
            for step in &scenario.steps {
                let call = capture_pg::PgStep {
                    kind: step.kind.clone(),
                    label: step.label.clone(),
                    input: step.input.clone(),
                };
                let mut context = StepContext {
                    pool: &pool,
                    started,
                    namer: &mut namer,
                };
                let mut outcome = run_step(&call, &mut context).await;
                if let Some(object) = outcome.result.as_mut().and_then(Json::as_object_mut)
                    && let Some(id) = object
                        .get("conversationId")
                        .and_then(Value::as_str)
                        .and_then(|raw| uuid::Uuid::parse_str(raw).ok())
                {
                    let placeholder = namer.conv(&id.to_string());
                    object.insert("conversationId".to_string(), Value::String(placeholder));
                }
                if let Some(rows) = outcome.rows.as_mut()
                    && let Some(object) = rows.as_object_mut()
                {
                    object.remove("graphile_jobs");
                }
                outcomes.push(outcome);
            }
            let mut remaining = outcomes.into_iter();
            let replay =
                |_step: &capture_pg::PgStep| -> PgOutcome { remaining.next().unwrap_or_default() };
            capture_pg::replay_scenario(&scenario, None, replay)
                .map_err(|mismatch| mismatch.detail)?;
        }
    }
    Ok(())
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn scenario_inventory_is_covered(_pool: PgPool) -> TestResult {
    let mut total = 0usize;
    let mut seen: Vec<String> = Vec::new();
    for file in GOLDEN_FILES {
        let path = testkit::recording(&format!("capture/pg/{file}"));
        for scenario in capture_pg::scenarios(&path)? {
            seen.push(format!("{} ({})", scenario.name, scenario.schema));
            total += 1;
        }
    }
    assert_eq!(total, 30, "recorded scenarios: {seen:?}");
    Ok(())
}

fn drop_graphile_rows(scenario: &mut capture_pg::ScenarioRecord) {
    for step in &mut scenario.steps {
        if let Some(rows) = step.rows.as_mut()
            && let Some(object) = rows.as_object_mut()
        {
            object.remove("graphile_jobs");
        }
    }
}

struct StepContext<'a> {
    pool: &'a PgPool,
    started: chrono::DateTime<chrono::Utc>,
    namer: &'a mut testkit::PgNamer,
}

async fn run_step(step: &capture_pg::PgStep, context: &mut StepContext<'_>) -> PgOutcome {
    let mut outcome = PgOutcome::default();
    match step.kind.as_str() {
        "sql" => {
            if step.label == "another writer creates the conversation without task_id"
                && let Err(error) = sqlx::query(
                    "INSERT INTO rh_conversations (session_key, agent) VALUES ('claude-code:spawn-three', 'rivet-claude')",
                )
                .execute(context.pool)
                .await
            {
                outcome.error = Some(error.to_string());
            }
        }
        "ingestHookEvent" => {
            let payload = at(&step.input, "payload");
            match ingest_hook_event(context.pool, payload, &step.input).await {
                Ok(result) => outcome.result = Some(result),
                Err(_) => outcome.error = Some("error".to_string()),
            }
        }
        "ingestTranscript" => {
            let path = at(&step.input, "transcriptPath").as_str().unwrap_or_default();
            let lines = at(&step.input, "transcriptLines").as_array();
            match ingest_transcript(context.pool, path, lines, at(&step.input, "options")).await {
                Ok(result) => outcome.result = Some(result),
                Err(_) => outcome.error = Some("error".to_string()),
            }
        }
        _ => {}
    }
    outcome.rows = Some(
        match dump_rivetos(context.pool, context.started, context.namer).await {
            Ok(rows) => rows,
            Err(mismatch) => encoded(DumpError {
                dump_error: format!("{mismatch}"),
            }),
        },
    );
    outcome
}

#[derive(Debug)]
struct IngestError;

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ingest replay failed")
    }
}

impl std::error::Error for IngestError {}

impl From<sqlx::Error> for IngestError {
    fn from(_: sqlx::Error) -> Self {
        Self
    }
}

impl From<memory_postgres::Error> for IngestError {
    fn from(_: memory_postgres::Error) -> Self {
        Self
    }
}

impl From<serde_json::Error> for IngestError {
    fn from(_: serde_json::Error) -> Self {
        Self
    }
}

const AGENT: &str = "rivet-claude";
const CHANNEL: &str = "claude-code";

fn str_at(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn now_stamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
}

fn encoded<T: serde::Serialize>(value: T) -> Json {
    serde_json::to_value(value).unwrap_or_default()
}

#[derive(serde::Serialize)]
struct HookOutcomeBody {
    #[serde(rename = "sessionKey")]
    session_key: String,
    #[serde(rename = "conversationId")]
    conversation_id: String,
    created: bool,
    inserted: u64,
    #[serde(rename = "alreadyStored", skip_serializing_if = "Option::is_none")]
    already_stored: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped: Option<String>,
}

#[derive(serde::Serialize)]
struct HookSettings<'a> {
    source: &'a str,
    last_event: &'a str,
    last_ingest_at: String,
    session_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
}

#[derive(serde::Serialize)]
struct TranscriptSettings {
    source: &'static str,
    file: String,
    session_id: Option<String>,
    pr_url: Option<String>,
    last_event: Option<String>,
    last_ingest_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
}

#[derive(serde::Serialize)]
struct DumpError {
    dump_error: String,
}

#[derive(serde::Serialize)]
struct HookEventMetadata {
    source: &'static str,
    hook_event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ingest_key: Option<String>,
    #[serde(flatten)]
    herdr: HerdrMetadata,
}

#[derive(serde::Serialize)]
struct TranscriptMetadata {
    source: &'static str,
    uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sidechain: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hook_event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovered: Option<bool>,
    #[serde(flatten)]
    herdr: HerdrMetadata,
}

#[derive(serde::Serialize, Default, Clone)]
struct HerdrMetadata {
    #[serde(rename = "herdr_pane_id", skip_serializing_if = "Option::is_none")]
    pane_id: Option<String>,
    #[serde(rename = "herdr_workspace_id", skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
    #[serde(rename = "herdr_host", skip_serializing_if = "Option::is_none")]
    host: Option<String>,
}

impl HerdrMetadata {
    fn from_opts(opts: &Value) -> Self {
        opts.get("herdr").map_or_else(Self::default, |herdr| Self {
            pane_id: str_at(herdr, "paneId"),
            workspace_id: str_at(herdr, "workspaceId"),
            host: str_at(herdr, "host"),
        })
    }
}

impl TranscriptMetadata {
    fn build(
        uuid: Option<&str>,
        sidechain: Option<bool>,
        hook_event: Option<&str>,
        recovered: Option<bool>,
        herdr: HerdrMetadata,
    ) -> Result<wire::JsonObject, IngestError> {
        Ok(serde_json::from_value(encoded(Self {
            source: "claude-code-hook",
            uuid: uuid.map(str::to_string),
            sidechain,
            hook_event: hook_event.map(str::to_string),
            recovered,
            herdr,
        }))?)
    }
}

fn parse_stamp(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|parsed| parsed.with_timezone(&chrono::Utc))
}

fn routed_user(opts: &Value) -> Option<String> {
    let raw = opts
        .get("env")
        .and_then(|env| str_at(env, "RIVETOS_USER_ID"))?;
    let trimmed = raw.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn task_id(opts: &Value) -> Option<String> {
    let raw = str_at(opts, "taskId")?;
    uuid::Uuid::parse_str(&raw).ok().map(|_| raw)
}

fn path_key(path: &str) -> String {
    let rel = path.strip_prefix("__FIXTURES__/").map_or_else(
        || path.to_string(),
        |rest| format!("__FIXTURES_REL__/{rest}"),
    );
    let rel = rel.strip_prefix('/').unwrap_or(&rel).to_string();
    let trimmed = rel.strip_suffix(".jsonl").unwrap_or(&rel).to_string();
    format!("claude-code:{trimmed}")
}

fn text_parts(content: &Value) -> (Option<String>, Vec<(String, String, Value)>) {
    let mut segments: Vec<String> = Vec::new();
    let mut calls: Vec<(String, String, Value)> = Vec::new();
    match content {
        Value::String(text) => {
            if !text.is_empty() {
                segments.push(text.clone());
            }
        }
        Value::Array(parts) => {
            for part in parts {
                match str_at(part, "type").unwrap_or_default().as_str() {
                    "text" => {
                        if let Some(text) = str_at(part, "text") {
                            segments.push(text);
                        }
                    }
                    "thinking" => {
                        if let Some(text) = str_at(part, "thinking") {
                            segments.push(format!("[thinking] {text}"));
                        }
                    }
                    "tool_use" => {
                        let id = str_at(part, "id").unwrap_or_default();
                        let name = str_at(part, "name").unwrap_or_default();
                        let input = part.get("input").cloned().unwrap_or(Value::Null);
                        calls.push((id, name, input));
                    }
                    _ => {}
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => {}
    }
    let text = if segments.is_empty() {
        None
    } else {
        Some(segments.join("\n"))
    };
    (text, calls)
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => {
            let mut segments: Vec<String> = Vec::new();
            for part in parts {
                match part {
                    Value::String(text) => segments.push(text.clone()),
                    Value::Object(object) => {
                        if str_at(part, "type").as_deref() == Some("text") {
                            if let Some(text) = str_at(part, "text") {
                                segments.push(text);
                            }
                        } else if let Some(text) = object.get("content").and_then(Value::as_str) {
                            segments.push(text.to_string());
                        }
                    }
                    Value::Null | Value::Bool(_) | Value::Number(_) | Value::Array(_) => {}
                }
            }
            segments.join("\n")
        }
        other @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_)) => {
            other.to_string()
        }
    }
}

const CONTENT_CAP: usize = 16_000;
const TITLE_CAP: usize = 120;
const TRUNCATION_MARKER: &str = "\n\u{2026}[truncated]";

fn capped(text: &str) -> String {
    match text.char_indices().nth(CONTENT_CAP) {
        Some((end, _)) => format!("{}{TRUNCATION_MARKER}", &text[..end]),
        None => text.to_owned(),
    }
}

fn title_of(text: &str) -> String {
    text.chars().take(TITLE_CAP).collect()
}

fn result_object(value: &Value) -> Result<Option<wire::JsonObject>, IngestError> {
    Ok(if value.is_object() {
        Some(serde_json::from_value(value.clone())?)
    } else {
        None
    })
}

fn tool_result_of(payload: &Value) -> Option<String> {
    let raw = match payload.get("tool_response") {
        None | Some(Value::Null) => str_at(payload, "tool_result")?,
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    };
    Some(capped(&raw))
}

#[expect(
    clippy::too_many_lines,
    reason = "one recorded hook narrative per call"
)]
async fn ingest_hook_event(
    pool: &PgPool,
    payload: &Value,
    opts: &Value,
) -> Result<Json, IngestError> {
    let event = str_at(payload, "hook_event_name").unwrap_or_default();
    let Some(session_id) = str_at(payload, "session_id") else {
        return Ok(encoded(HookOutcomeBody {
            session_key: String::new(),
            conversation_id: String::new(),
            created: false,
            inserted: 0,
            already_stored: None,
            skipped: Some("no session_id".to_string()),
        }));
    };
    if event != "UserPromptSubmit" && event != "PostToolUse" {
        return Ok(encoded(HookOutcomeBody {
            session_key: format!("claude-code:{session_id}"),
            conversation_id: String::new(),
            created: false,
            inserted: 0,
            already_stored: None,
            skipped: Some(format!("unhandled event {event}")),
        }));
    }
    let session_key =
        str_at(opts, "sessionKeyOverride").unwrap_or_else(|| format!("claude-code:{session_id}"));

    let prompt = str_at(payload, "prompt");
    if event == "UserPromptSubmit" && prompt.as_deref().is_none_or(|p| p.trim().is_empty()) {
        return Ok(encoded(HookOutcomeBody {
            session_key: session_key.clone(),
            conversation_id: String::new(),
            created: false,
            inserted: 0,
            already_stored: None,
            skipped: Some("empty prompt".to_string()),
        }));
    }

    let title = match event.as_str() {
        "UserPromptSubmit" => title_of(prompt.as_deref().unwrap_or_default()),
        _ => "Claude Code session".to_string(),
    };
    let settings: wire::JsonObject = serde_json::from_value(encoded(HookSettings {
        source: "claude-code-hook",
        last_event: &event,
        last_ingest_at: now_stamp(),
        session_id: &session_id,
        cwd: str_at(payload, "cwd").map(|cwd| cwd.trim().to_string()),
    }))?;

    let mut writer = memory_postgres::CaptureWriter::begin(pool, &session_key).await?;
    let owner = routed_user(opts);
    let record = writer
        .ensure_conversation(&memory_postgres::ConversationEnsure {
            session_key: &session_key,
            agent: AGENT,
            channel: Some(CHANNEL),
            title: Some(&title),
            settings: &settings,
            task_id: task_id(opts).as_deref(),
            owner_user_id: owner.as_deref(),
            created_at: None,
            updated_at: None,
            active: true,
        })
        .await?;

    let idempotency = str_at(opts, "idempotencyKey");
    let mut skipped: Option<String> = None;
    let mut inserted = 0u64;
    let duplicate = match &idempotency {
        Some(key) => writer.ingest_key_seen(record.id, key).await?,
        None => false,
    };
    if duplicate {
        skipped = Some("duplicate ingest_key".to_string());
    } else {
        let mut metadata = HookEventMetadata {
            source: "claude-code-hook",
            hook_event: event.clone(),
            ingest_key: None,
            herdr: HerdrMetadata::from_opts(opts),
        };
        if let Some(key) = &idempotency {
            metadata.ingest_key = Some(key.clone());
        }
        let metadata: wire::JsonObject = serde_json::from_value(encoded(metadata))?;
        let prompt_content;
        let tool_name;
        let tool_content;
        let args;
        let result;
        let row = if event.as_str() == "UserPromptSubmit" {
            prompt_content = capped(prompt.as_deref().unwrap_or_default());
            memory_postgres::MessageRow {
                conversation_id: record.id,
                agent: AGENT,
                channel: CHANNEL,
                role: wire::message::Role::User,
                content: &prompt_content,
                tool_name: None,
                tool_args: None,
                tool_result: None,
                metadata: &metadata,
                owner_user_id: owner.as_deref(),
                created_at: None,
            }
        } else {
            tool_name = str_at(payload, "tool_name").unwrap_or_else(|| "unknown".into());
            tool_content = format!("[tool call] {tool_name}");
            args = result_object(payload.get("tool_input").unwrap_or(&Value::Null))?;
            result = tool_result_of(payload);
            memory_postgres::MessageRow {
                conversation_id: record.id,
                agent: AGENT,
                channel: CHANNEL,
                role: wire::message::Role::Tool,
                content: &tool_content,
                tool_name: Some(&tool_name),
                tool_args: args.as_ref(),
                tool_result: result.as_deref(),
                metadata: &metadata,
                owner_user_id: owner.as_deref(),
                created_at: None,
            }
        };
        writer.insert_message(&row).await?;
        inserted = 1;
    }
    writer.touch_settings(record.id, &settings).await?;
    if let Some(owner_name) = &owner
        && !record.created
    {
        writer.adopt_owner(record.id, owner_name).await?;
    }
    writer.commit().await?;

    Ok(encoded(HookOutcomeBody {
        session_key,
        conversation_id: record.id.to_string(),
        created: record.created,
        inserted,
        already_stored: None,
        skipped,
    }))
}

struct Transcript {
    assistants: Vec<AssistantLine>,
    users: Vec<UserLine>,
    results: std::collections::HashMap<String, String>,
    ai_titles: Vec<String>,
    pr_url: Option<String>,
    cwd: Option<String>,
    session_id: Option<String>,
    stamps: Vec<chrono::DateTime<chrono::Utc>>,
    ingestable: bool,
}

struct AssistantLine {
    uuid: Option<String>,
    sidechain: bool,
    text: Option<String>,
    calls: Vec<(String, String, Value)>,
    at: Option<chrono::DateTime<chrono::Utc>>,
}

struct UserLine {
    uuid: Option<String>,
    content: String,
    at: Option<chrono::DateTime<chrono::Utc>>,
}

#[expect(
    clippy::too_many_lines,
    reason = "one recorded transcript grammar per pass"
)]
fn parse_lines(lines: &[Value]) -> Transcript {
    let mut parsed = Transcript {
        assistants: Vec::new(),
        users: Vec::new(),
        results: std::collections::HashMap::new(),
        ai_titles: Vec::new(),
        pr_url: None,
        cwd: None,
        session_id: None,
        stamps: Vec::new(),
        ingestable: false,
    };
    for value in lines {
        if !value.is_object() {
            continue;
        }
        let kind = str_at(value, "type").unwrap_or_default();
        if let Some(session) = str_at(value, "sessionId")
            && parsed.session_id.is_none()
        {
            parsed.session_id = Some(session);
        }
        if let Some(cwd) = str_at(value, "cwd") {
            parsed.cwd = Some(cwd.trim().to_string());
        }
        match kind.as_str() {
            "ai-title" => {
                if let Some(title) = str_at(value, "aiTitle") {
                    parsed.ai_titles.push(title);
                }
            }
            "pr-link" => {
                parsed.pr_url = str_at(value, "prUrl");
            }
            "user" => {
                if value.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let Some(message) = value.get("message") else {
                    continue;
                };
                let uuid = str_at(value, "uuid");
                let at = str_at(value, "timestamp").and_then(|t| parse_stamp(&t));
                if let Some(stamp) = at {
                    parsed.stamps.push(stamp);
                }
                match message.get("content") {
                    Some(Value::String(text)) => {
                        parsed.users.push(UserLine {
                            uuid,
                            content: text.clone(),
                            at,
                        });
                        parsed.ingestable = true;
                    }
                    Some(Value::Array(parts)) => {
                        for part in parts {
                            if str_at(part, "type").as_deref() == Some("tool_result") {
                                let id = str_at(part, "tool_use_id").unwrap_or_default();
                                let text = result_text(part.get("content").unwrap_or(&Value::Null));
                                parsed.results.insert(id, text);
                            }
                        }
                    }
                    _ => {}
                }
            }
            "assistant" => {
                let Some(message) = value.get("message") else {
                    continue;
                };
                let (text, calls) = text_parts(message.get("content").unwrap_or(&Value::Null));
                let at = str_at(value, "timestamp").and_then(|t| parse_stamp(&t));
                if let Some(stamp) = at {
                    parsed.stamps.push(stamp);
                }
                if text.is_some() || !calls.is_empty() {
                    parsed.ingestable = true;
                }
                parsed.assistants.push(AssistantLine {
                    uuid: str_at(value, "uuid"),
                    sidechain: value.get("isSidechain").and_then(Value::as_bool) == Some(true),
                    text,
                    calls,
                    at,
                });
            }
            _ => {}
        }
    }
    parsed
}

#[expect(
    clippy::too_many_lines,
    reason = "the transcript ingest is one narrative"
)]
async fn ingest_transcript(
    pool: &PgPool,
    path: &str,
    lines: Option<&Vec<Value>>,
    opts: &Value,
) -> Result<Json, IngestError> {
    let Some(lines) = lines else {
        return Ok(encoded(HookOutcomeBody {
            session_key: path_key(path),
            conversation_id: String::new(),
            created: false,
            inserted: 0,
            already_stored: Some(0),
            skipped: Some("transcript file does not exist".to_string()),
        }));
    };
    let decoded: Vec<Value> = lines
        .iter()
        .filter_map(|line| {
            line.as_str()
                .and_then(|text| serde_json::from_str(text).ok())
        })
        .collect();
    let parsed = parse_lines(&decoded);
    if !parsed.ingestable {
        return Ok(encoded(HookOutcomeBody {
            session_key: path_key(path),
            conversation_id: String::new(),
            created: false,
            inserted: 0,
            already_stored: Some(0),
            skipped: Some("nothing to ingest".to_string()),
        }));
    }
    let session_id = str_at(opts, "sessionId").or_else(|| parsed.session_id.clone());
    let session_key = match str_at(opts, "sessionKeyOverride") {
        Some(override_key) => override_key,
        None => match &session_id {
            Some(id) => format!("claude-code:{id}"),
            None => path_key(path),
        },
    };
    let settings: wire::JsonObject = serde_json::from_value(encoded(TranscriptSettings {
        source: "claude-code-hook",
        file: path.to_string(),
        session_id: session_id.clone(),
        pr_url: parsed.pr_url.clone(),
        last_event: str_at(opts, "event"),
        last_ingest_at: now_stamp(),
        cwd: parsed.cwd.clone(),
    }))?;

    let fallback_title = parsed.users.first().map(|user| user.content.clone());
    let latest_ai_title = parsed.ai_titles.last().cloned();
    let title_for_create = latest_ai_title
        .clone()
        .or_else(|| fallback_title.as_deref().map(title_of));

    let mut writer = memory_postgres::CaptureWriter::begin(pool, &session_key).await?;
    let owner = routed_user(opts);
    let herdr = HerdrMetadata::from_opts(opts);
    let record = writer
        .ensure_conversation(&memory_postgres::ConversationEnsure {
            session_key: &session_key,
            agent: AGENT,
            channel: Some(CHANNEL),
            title: title_for_create.as_deref(),
            settings: &settings,
            task_id: task_id(opts).as_deref(),
            owner_user_id: owner.as_deref(),
            created_at: parsed.stamps.first().copied(),
            updated_at: parsed.stamps.last().copied(),
            active: opts.get("markInactive").and_then(Value::as_bool) != Some(true),
        })
        .await?;

    let mut inserted = 0u64;
    let mut already_stored = 0u64;

    let stored_uuids = writer.stored_assistant_uuids(record.id).await?;
    for line in &parsed.assistants {
        let Some(text) = &line.text else { continue };
        if line
            .uuid
            .as_ref()
            .is_some_and(|uuid| stored_uuids.contains(uuid))
        {
            already_stored += 1;
            continue;
        }
        let metadata = TranscriptMetadata::build(
            line.uuid.as_deref(),
            Some(line.sidechain),
            None,
            None,
            herdr.clone(),
        )?;
        writer
            .insert_message(&memory_postgres::MessageRow {
                conversation_id: record.id,
                agent: AGENT,
                channel: CHANNEL,
                role: wire::message::Role::Assistant,
                content: &capped(text),
                tool_name: None,
                tool_args: None,
                tool_result: None,
                metadata: &metadata,
                owner_user_id: owner.as_deref(),
                created_at: line.at,
            })
            .await?;
        inserted += 1;
    }

    let mut supply: Vec<(Option<String>, Option<Value>)> = writer
        .stored_tool_calls(record.id)
        .await?
        .into_iter()
        .map(|(name, args)| (name, args.and_then(|text| serde_json::from_str(&text).ok())))
        .collect();
    for line in &parsed.assistants {
        for (call_id, name, input) in &line.calls {
            let result = parsed.results.get(call_id);
            if let Some(found) = supply.iter().position(|(stored, args)| {
                stored.as_deref().unwrap_or_default() == name
                    && args
                        .as_ref()
                        .is_some_and(|args| testkit::json_eq(args, input))
            }) {
                supply.swap_remove(found);
                already_stored += 1;
                continue;
            }
            let metadata = TranscriptMetadata::build(
                line.uuid.as_deref(),
                None,
                Some("PostToolUse"),
                Some(true),
                herdr.clone(),
            )?;
            let args: wire::JsonObject = serde_json::from_value(input.clone())?;
            writer
                .insert_message(&memory_postgres::MessageRow {
                    conversation_id: record.id,
                    agent: AGENT,
                    channel: CHANNEL,
                    role: wire::message::Role::Tool,
                    content: &format!("[tool call] {name}"),
                    tool_name: Some(name),
                    tool_args: Some(&args),
                    tool_result: result.map(String::as_str),
                    metadata: &metadata,
                    owner_user_id: owner.as_deref(),
                    created_at: line.at,
                })
                .await?;
            inserted += 1;
        }
    }

    let stored_users = writer.stored_user_contents(record.id).await?;
    let mut skipped_users = 0u64;
    for line in &parsed.users {
        let content = capped(&line.content);
        if stored_users.contains(&content) {
            skipped_users += 1;
            continue;
        }
        let metadata =
            TranscriptMetadata::build(line.uuid.as_deref(), None, None, Some(true), herdr.clone())?;
        writer
            .insert_message(&memory_postgres::MessageRow {
                conversation_id: record.id,
                agent: AGENT,
                channel: CHANNEL,
                role: wire::message::Role::User,
                content: &content,
                tool_name: None,
                tool_args: None,
                tool_result: None,
                metadata: &metadata,
                owner_user_id: owner.as_deref(),
                created_at: line.at,
            })
            .await?;
        inserted += 1;
    }
    already_stored += skipped_users;
    already_stored += u64::try_from(
        stored_users
            .len()
            .saturating_sub(usize::try_from(skipped_users).unwrap_or(usize::MAX)),
    )
    .unwrap_or(0);

    let title_for_update = latest_ai_title.or_else(|| {
        if record.created {
            None
        } else {
            record.title.clone()
        }
    });
    let mark_inactive = opts.get("markInactive").and_then(Value::as_bool) == Some(true);
    writer
        .update_conversation(&memory_postgres::ConversationUpdate {
            id: record.id,
            updated_at: parsed.stamps.last().copied(),
            active: !mark_inactive,
            title: title_for_update.as_deref(),
            settings: &settings,
        })
        .await?;
    if let Some(owner_name) = &owner
        && !record.created
    {
        writer.adopt_owner(record.id, owner_name).await?;
    }
    writer.commit().await?;

    Ok(encoded(HookOutcomeBody {
        session_key,
        conversation_id: record.id.to_string(),
        created: record.created,
        inserted,
        already_stored: Some(already_stored),
        skipped: None,
    }))
}
