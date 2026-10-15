#[expect(
    dead_code,
    reason = "integration binaries share fixtures and use different subsets"
)]
mod common;

use std::str::FromStr as _;

use chrono::DateTime;
use chrono::Utc;
use common::Fresh;
use common::Ids;
use common::TestResult;
use common::batch;
use common::deploy_batch;
use common::expect_json;
use common::expect_logs;
use common::fresh_store;
use common::instant;
use common::message;
use kernel::memory::Limit;
use kernel::memory::SearchMode;
use kernel::memory::SearchOutcome;
use kernel::memory::SearchQuery;
use kernel::memory::TagLiteral;
use kernel::memory::TagLiteralError;
use memory_sqlite::SqliteError;
use memory_sqlite::SqliteStore;
use rusqlite::params;
use serde::Serialize;
use testkit::Json;
use wire::capture::CaptureBatchRequest;
use wire::gateway::memory::BrowseWindow;
use wire::gateway::memory::MemoryHitSource;
use wire::gateway::memory::MemoryScope;
use wire::memory::MemorySearchResult;
use wire::message::Role;

const CLOCK: &str = "2026-10-10T12:03:00.000Z";
const POOL_CLOCK: &str = "2026-10-10T12:00:00.000Z";
const LEAF_ID: &str = "00000000-0000-4000-8000-400000000001";
const BRANCH_ID: &str = "00000000-0000-4000-8000-400000000002";
const LEAF: &str = "The team decided to deploy the AcmeKit service with a blue-green rollout. The release branch is cut on Mondays, the deploy script runs from the release branch, and a health check on staging.example.com gates the traffic switch. Rollback is a traffic switch back to the previous colour.";
const BRANCH: &str = "Branch: staging deploys and database migration plans for AcmeKit";
const TAGGED_NOTE: &str =
    "a long note that mentions the rollback only once among many other unrelated words here";

const IN_SCOPE: &[&str] = &["runtime", "fts_only", "tag_pool_widening"];

const SKIPPED_SEARCH_SCENARIOS: &[(&str, &str)] = &[
    (
        "helpers",
        "buildFtsMatchQuery, relevanceFromBm25 and resolveTaskId are TypeScript internals, not a search scenario",
    ),
    (
        "hybrid",
        "vector search, the literal arm, trigram fallback and degraded query embedding belong to the embedding slice",
    ),
];

#[test]
fn skipped_search_scenarios_are_named() -> TestResult {
    let file: Json = common::recording("search.json")?;
    let keys = file.as_object().ok_or("search.json is not an object")?;
    for key in keys.keys() {
        let named = IN_SCOPE.contains(&key.as_str())
            || SKIPPED_SEARCH_SCENARIOS
                .iter()
                .any(|(name, _reason)| *name == key);
        if !named {
            return Err(format!("search.json key {key} is neither replayed nor skipped").into());
        }
    }
    for (name, reason) in SKIPPED_SEARCH_SCENARIOS {
        if reason.is_empty() || keys.get(*name).is_none() {
            return Err(format!("skipped scenario {name} needs a reason and a recording").into());
        }
    }
    Ok(())
}

#[tokio::test]
async fn fts_only_replay_matches_the_recording() -> TestResult {
    let file: Json = common::recording("search.json")?;
    let fts = file.get("fts_only").ok_or("fts_only")?;
    let fresh = seed_fts().await?;
    let mut ids = Ids::default();
    let clock = instant(field_str(fts, "clock")?)?;
    let notice = recorded_notice(fts)?;
    replay_memory(&fresh.store, fts, clock, notice, &mut ids).await?;
    replay_backend(&fresh.store, fts, clock, &mut ids).await?;
    expect_access(&fresh.store, fts, &mut ids).await?;
    let logs = string_list(fts.get("logs").ok_or("logs")?)?;
    expect_logs(&logs, &fresh.store.logs(), "fts_only logs", &mut ids)?;
    last_seven_days_keeps_the_october_rollback(&fresh.store, clock).await?;
    Ok(())
}

#[tokio::test]
async fn tag_pool_widening_returns_the_buried_hit() -> TestResult {
    let file: Json = common::recording("search.json")?;
    let pool = file.get("tag_pool_widening").ok_or("tag_pool_widening")?;
    let fresh = fresh_store(None)?;
    let now = instant(POOL_CLOCK)?;
    seed_tag_pool(&fresh.store, now).await?;
    let step = pool
        .get("steps")
        .and_then(Json::as_array)
        .and_then(|steps| steps.first())
        .ok_or("tag pool step")?;
    let result = step.get("result").ok_or("tag pool result")?;
    let query = asked("rollback", MemoryScope::Messages, 2);
    let mut query = query;
    query.tag = Some("project:rare".parse()?);
    let outcome = fresh.store.search_at(query, now).await?;
    let mut ids = Ids::default();
    expect_degraded(
        result.get("degraded").ok_or("degraded")?,
        &outcome,
        &mut ids,
        "tag pool degraded",
    )?;
    expect_hits(
        result.get("results").ok_or("results")?,
        &outcome.results,
        &mut ids,
        "tag pool hits",
    )?;
    let logs = string_list(pool.get("logs").ok_or("logs")?)?;
    expect_logs(&logs, &fresh.store.logs(), "tag pool logs", &mut ids)?;
    Ok(())
}

#[tokio::test]
async fn keyword_search_rejects_the_other_modes() -> TestResult {
    let fresh = fresh_store(None)?;
    let now = instant(CLOCK)?;
    for mode in [SearchMode::Fts, SearchMode::Hybrid, SearchMode::Vector] {
        let mut query = asked("deploy", MemoryScope::Both, 10);
        query.mode = mode;
        let outcome = fresh.store.search_at(query, now).await?;
        assert!(outcome.results.is_empty(), "{mode}");
        assert!(outcome.degraded.is_some(), "{mode}");
        assert_eq!(outcome.expanded, []);
        assert!(outcome.fallback.is_none());
        assert!(outcome.synthesis.is_none());
    }
    fresh.store.arm_embed();
    let armed = fresh
        .store
        .search_at(asked("deploy", MemoryScope::Both, 10), now)
        .await?;
    assert!(armed.degraded.is_none());
    for mode in [SearchMode::Trigram, SearchMode::Regex] {
        let mut query = asked("deploy", MemoryScope::Both, 10);
        query.mode = mode;
        let error = fresh
            .store
            .search_at(query, now)
            .await
            .expect_err("keyword index");
        assert_eq!(
            error.to_string(),
            format!("search mode {mode} is not answered by the keyword index")
        );
    }
    let mut synthesize = asked("deploy", MemoryScope::Both, 10);
    synthesize.synthesize = Some(true);
    let error = fresh
        .store
        .search_at(synthesize, now)
        .await
        .expect_err("synthesis");
    assert_eq!(
        error.to_string(),
        "memory synthesis requires a configured summarizer endpoint"
    );
    let mut inverted = asked("deploy", MemoryScope::Both, 10);
    let stamp = now;
    inverted.since = Some(stamp);
    inverted.before = Some(stamp);
    let error = fresh
        .store
        .search_at(inverted, now)
        .await
        .expect_err("range");
    assert_eq!(error.to_string(), "since must precede before");
    Ok(())
}

async fn seed_fts() -> TestResult<Fresh> {
    let fresh = fresh_store(None)?;
    let now = instant(common::RECORDED_START)?;
    let deployed = fresh
        .store
        .capture(deploy_batch("cli:sess-1", "rivet")?, now)
        .await?;
    let helper = fresh.store.capture(helper_batch()?, now).await?;
    fresh.store.capture(rollback_batch()?, now).await?;
    let sess1 = deployed.conversation_id;
    let sess2 = helper.conversation_id;
    fresh
        .store
        .with_conn(move |conn| seed_rows(conn, &sess1, &sess2))
        .await?;
    Ok(fresh)
}

fn helper_batch() -> TestResult<CaptureBatchRequest> {
    let mut note = message(
        "h-note",
        Role::User,
        "notes about the staging database migration on db.example.com",
    );
    note.created_at = Some(instant("2026-10-10T12:01:00.000Z")?);
    let mut helper = batch("cli:sess-2", "helper", "cli");
    helper.messages = vec![
        note,
        message("h-fill", Role::User, &filler_release()),
        message("h-empty-1", Role::User, ""),
        message("h-empty-2", Role::User, ""),
    ];
    Ok(helper)
}

fn filler_release() -> String {
    let mut words: Vec<String> = (0..30).map(|index| format!("zz{index}")).collect();
    words.push("release".to_owned());
    words.join(" ")
}

fn rollback_batch() -> TestResult<CaptureBatchRequest> {
    let mut user = message(
        "rb-user",
        Role::User,
        "rollback rollback rollback procedure notes",
    );
    user.created_at = Some(instant("2026-09-01T00:00:00.000Z")?);
    let mut assistant = message(
        "rb-assistant",
        Role::Assistant,
        "the rollback switches traffic back to the previous colour",
    );
    assistant.created_at = Some(instant("2026-10-09T00:00:00.000Z")?);
    let mut rollback = batch("cli:sess-3", "rivet", "cli");
    rollback.messages = vec![user, assistant];
    Ok(rollback)
}

fn seed_rows(conn: &rusqlite::Connection, sess1: &str, sess2: &str) -> Result<(), SqliteError> {
    let stamp = common::RECORDED_START;
    conn.execute(
        "INSERT INTO ros_summaries (id, conversation_id, content, kind, created_at)
         VALUES (?1, ?2, ?3, 'leaf', '2026-10-10T12:00:30.000Z')",
        params![LEAF_ID, sess1, LEAF],
    )
    .map_err(|source| SqliteError::Query {
        operation: "seed leaf",
        source,
    })?;
    conn.execute(
        "INSERT INTO ros_summaries (id, conversation_id, content, kind, created_at)
         VALUES (?1, NULL, ?2, 'branch', '2026-10-10T12:00:40.000Z')",
        params![BRANCH_ID, BRANCH],
    )
    .map_err(|source| SqliteError::Query {
        operation: "seed branch",
        source,
    })?;
    insert_tag(
        conn,
        "tag-acme",
        "conversation",
        sess1,
        ("project", "acmekit"),
        stamp,
    )?;
    insert_tag(
        conn,
        "tag-infra",
        "conversation",
        sess2,
        ("project", "infra"),
        stamp,
    )?;
    insert_tag(
        conn,
        "tag-topic",
        "summary",
        LEAF_ID,
        ("topic", "deploys"),
        stamp,
    )?;
    Ok(())
}

fn insert_tag(
    conn: &rusqlite::Connection,
    id: &str,
    entity_type: &str,
    entity_id: &str,
    tag: (&str, &str),
    stamp: &str,
) -> Result<(), SqliteError> {
    conn.execute(
        "INSERT INTO ros_tags
            (id, entity_type, entity_id, key, value, source, state, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'user', 'accepted', ?6, ?6)",
        params![id, entity_type, entity_id, tag.0, tag.1, stamp],
    )
    .map_err(|source| SqliteError::Query {
        operation: "seed tag",
        source,
    })?;
    Ok(())
}

async fn seed_tag_pool(store: &SqliteStore, now: DateTime<Utc>) -> TestResult {
    let tagged = store
        .capture(one_message("tagged", TAGGED_NOTE), now)
        .await?;
    store
        .with_conn({
            let conversation = tagged.conversation_id.clone();
            move |conn| {
                insert_tag(
                    conn,
                    "tag-rare",
                    "conversation",
                    &conversation,
                    ("project", "rare"),
                    POOL_CLOCK,
                )
            }
        })
        .await?;
    for index in 0..20 {
        store
            .capture(one_message(&format!("short-{index}"), "rollback"), now)
            .await?;
    }
    for index in 0..20 {
        let pad = (0..9)
            .map(|word| format!("pad{index}x{word}"))
            .collect::<Vec<_>>()
            .join(" ");
        store
            .capture(one_message(&format!("pad-{index}"), &pad), now)
            .await?;
    }
    Ok(())
}

fn one_message(session_key: &str, content: &str) -> CaptureBatchRequest {
    let mut request = batch(session_key, "rivet", "cli");
    request.messages = vec![message(session_key, Role::User, content)];
    request
}

async fn replay_memory(
    store: &SqliteStore,
    fts: &Json,
    clock: DateTime<Utc>,
    notice: &Json,
    ids: &mut Ids,
) -> TestResult {
    let cases = array(fts, "memorySearch")?;
    for case in cases {
        let text = field_str(case, "query")?;
        let outcome = store
            .search_at(
                recorded_query(text, case.get("options").ok_or("options")?)?,
                clock,
            )
            .await?;
        assert_slice(&outcome);
        expect_degraded(notice, &outcome, ids, &format!("memory {text} degraded"))?;
        expect_hits(
            case.get("hits").ok_or("hits")?,
            &outcome.results,
            ids,
            &format!("memory {text}"),
        )?;
    }
    Ok(())
}

async fn replay_backend(
    store: &SqliteStore,
    fts: &Json,
    clock: DateTime<Utc>,
    ids: &mut Ids,
) -> TestResult {
    for case in array(fts, "backendSearch")? {
        let text = field_str(case, "query")?;
        let options = case.get("options").ok_or("options")?;
        if let Some(error) = case.get("error") {
            expect_rejected_tag(options, error)?;
            continue;
        }
        let response = case.get("response").ok_or("response")?;
        let outcome = store
            .search_at(recorded_query(text, options)?, clock)
            .await?;
        assert_slice(&outcome);
        expect_degraded(
            response.get("degraded").ok_or("degraded")?,
            &outcome,
            ids,
            &format!("backend {text} degraded"),
        )?;
        expect_hits(
            response.get("results").ok_or("results")?,
            &outcome.results,
            ids,
            &format!("backend {text}"),
        )?;
    }
    Ok(())
}

fn expect_rejected_tag(options: &Json, error: &Json) -> TestResult {
    let raw = field_str(options, "tag")?;
    let recorded = field_str(error, "message")?;
    let Err(rejected) = TagLiteral::from_str(raw) else {
        return Err(format!("tag {raw} was accepted").into());
    };
    let message = rejected.to_string();
    match rejected {
        TagLiteralError::Filter { raw: got } if got == raw => {}
        TagLiteralError::Filter { raw: got } | TagLiteralError::Literal { raw: got } => {
            return Err(format!("tag error for {got}").into());
        }
    }
    if message.starts_with(recorded) {
        Ok(())
    } else {
        Err(format!("tag error {message} does not start with {recorded}").into())
    }
}

async fn last_seven_days_keeps_the_october_rollback(
    store: &SqliteStore,
    clock: DateTime<Utc>,
) -> TestResult {
    let mut query = asked("rollback", MemoryScope::Messages, 10);
    query.window = Some(BrowseWindow::Last7d);
    let outcome = store.search_at(query, clock).await?;
    let contents: Vec<&str> = outcome
        .results
        .iter()
        .map(|hit| hit.content.as_str())
        .collect();
    assert_eq!(
        contents,
        ["the rollback switches traffic back to the previous colour"]
    );
    Ok(())
}

fn assert_slice(outcome: &SearchOutcome) {
    assert_eq!(outcome.expanded, []);
    assert!(outcome.fallback.is_none());
    assert!(outcome.synthesis.is_none());
}

fn recorded_query(text: &str, options: &Json) -> TestResult<SearchQuery> {
    let mut query = asked(text, scope_of(options)?, limit_of(options)?);
    if let Some(agent) = options.get("agent").and_then(Json::as_str) {
        query.agent = Some(agent.to_owned());
    }
    if let Some(raw) = options.get("tag").and_then(Json::as_str) {
        query.tag = Some(raw.parse()?);
    }
    Ok(query)
}

fn asked(text: &str, scope: MemoryScope, limit: u32) -> SearchQuery {
    SearchQuery {
        text: text.to_owned(),
        mode: SearchMode::Hybrid,
        scope,
        agent: None,
        tag: None,
        limit: Limit::clamped(Some(limit), 10, 1..=50),
        since: None,
        before: None,
        window: None,
        expand: None,
        synthesize: None,
    }
}

fn scope_of(options: &Json) -> TestResult<MemoryScope> {
    match options.get("scope").and_then(Json::as_str) {
        None | Some("both") => Ok(MemoryScope::Both),
        Some("messages") => Ok(MemoryScope::Messages),
        Some("summaries") => Ok(MemoryScope::Summaries),
        Some(other) => Err(format!("scope {other}").into()),
    }
}

fn limit_of(options: &Json) -> TestResult<u32> {
    match options.get("limit").and_then(Json::as_u64) {
        None => Ok(10),
        Some(limit) => {
            u32::try_from(limit).map_err(|error| format!("limit {limit}: {error}").into())
        }
    }
}

fn expect_hits(
    recorded: &Json,
    actual: &[MemorySearchResult],
    ids: &mut Ids,
    at: &str,
) -> TestResult {
    let hits = recorded.as_array().ok_or("hits are not an array")?;
    let expected: Vec<MemorySearchResult> =
        hits.iter().map(project_hit).collect::<TestResult<_>>()?;
    expect_json(&serde_json::to_value(expected)?, &codec(actual)?, at, ids)
}

fn codec<T: Serialize + ?Sized>(value: &T) -> TestResult<Json> {
    Ok(serde_json::from_str(&serde_json::to_string(value)?)?)
}

fn project_hit(hit: &Json) -> TestResult<MemorySearchResult> {
    let source = match field_str(hit, "layer").or_else(|_| field_str(hit, "source"))? {
        "message" => MemoryHitSource::Message,
        "summary" => MemoryHitSource::Summary,
        other => return Err(format!("source {other}").into()),
    };
    let content = field_str(hit, "content")?.to_owned();
    let role = projected_role(hit, source, &content)?;
    let relevance_score = hit
        .get("relevanceScore")
        .or_else(|| hit.get("score"))
        .and_then(Json::as_f64)
        .ok_or("missing score")?;
    Ok(MemorySearchResult {
        id: field_str(hit, "id")?.to_owned(),
        source,
        content,
        role,
        agent: match hit.get("agent") {
            None | Some(Json::Null) => None,
            Some(Json::String(agent)) => Some(agent.clone()),
            Some(other) => return Err(format!("agent {other}").into()),
        },
        relevance_score,
        created_at: instant(field_str(hit, "createdAt")?)?,
    })
}

fn projected_role(hit: &Json, source: MemoryHitSource, content: &str) -> TestResult<Option<Role>> {
    let recorded = field_str(hit, "role")?;
    match source {
        MemoryHitSource::Message => Ok(Some(recorded.parse()?)),
        MemoryHitSource::Summary => {
            let kind_matches = match recorded {
                "leaf" => content.starts_with("The team decided"),
                "branch" => content.starts_with("Branch:"),
                _ => false,
            };
            if kind_matches {
                Ok(None)
            } else {
                Err(format!("summary role {recorded} does not match its content").into())
            }
        }
    }
}

fn expect_degraded(
    recorded: &Json,
    outcome: &SearchOutcome,
    ids: &mut Ids,
    at: &str,
) -> TestResult {
    expect_json(recorded, &serde_json::to_value(&outcome.degraded)?, at, ids)
}

async fn expect_access(store: &SqliteStore, fts: &Json, ids: &mut Ids) -> TestResult {
    let messages = access_table(
        store,
        "SELECT id, access_count, last_accessed_at FROM ros_messages ORDER BY rowid",
    )
    .await?;
    let summaries = access_table(
        store,
        "SELECT id, access_count, last_accessed_at FROM ros_summaries ORDER BY rowid",
    )
    .await?;
    expect_json(
        fts.get("access_after").ok_or("access_after")?,
        &messages,
        "access_after",
        ids,
    )?;
    expect_json(
        fts.get("summary_access_after")
            .ok_or("summary_access_after")?,
        &summaries,
        "summary_access_after",
        ids,
    )
}

async fn access_table(store: &SqliteStore, sql: &str) -> TestResult<Json> {
    let sql = sql.to_owned();
    let rows = store
        .with_conn(move |conn| {
            let mut statement = conn.prepare(&sql).map_err(|source| SqliteError::Query {
                operation: "access",
                source,
            })?;
            let loaded = statement
                .query_map([], |row| {
                    Ok(AccessRow {
                        id: row.get(0)?,
                        access_count: row.get(1)?,
                        last_accessed_at: row.get(2)?,
                    })
                })
                .map_err(|source| SqliteError::Query {
                    operation: "access",
                    source,
                })?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|source| SqliteError::Query {
                    operation: "access",
                    source,
                })?;
            Ok(loaded)
        })
        .await?;
    codec(&rows)
}

#[derive(Serialize)]
struct AccessRow {
    id: String,
    access_count: i64,
    last_accessed_at: Option<String>,
}

fn recorded_notice(fts: &Json) -> TestResult<&Json> {
    for case in array(fts, "backendSearch")? {
        if let Some(notice) = case
            .get("response")
            .and_then(|response| response.get("degraded"))
        {
            return Ok(notice);
        }
    }
    Err("backend search recorded no degraded notice".into())
}

fn array<'a>(value: &'a Json, key: &str) -> TestResult<&'a Vec<Json>> {
    value
        .get(key)
        .and_then(Json::as_array)
        .ok_or_else(|| format!("missing {key}").into())
}

fn field_str<'a>(value: &'a Json, key: &str) -> TestResult<&'a str> {
    value
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("missing {key}").into())
}

fn string_list(value: &Json) -> TestResult<Vec<String>> {
    value
        .as_array()
        .ok_or("expected a string array")?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| "expected a string".into())
        })
        .collect()
}
