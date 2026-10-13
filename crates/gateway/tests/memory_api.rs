mod common;

use std::sync::Arc;

use common::answer;
use futures::executor::block_on;
use kernel::memory::Limit;
use kernel::memory::MemoryStore;
use testkit::Request;
use wire::gateway::GatewayError;
use wire::gateway::memory::MemoryBrowseResponse;
use wire::gateway::memory::MemoryBrowseToolArgs;
use wire::gateway::memory::MemoryGetFullToolArgs;
use wire::gateway::memory::MemorySearchResponse;
use wire::gateway::memory::MemorySearchToolArgs;
use wire::gateway::memory::MemoryTagsAddRequest;
use wire::gateway::memory::MemoryTagsDecideRequest;
use wire::gateway::memory::MemoryTagsLookupRequest;
use wire::gateway::memory::MemoryToolResponse;
use wire::gateway::memory::PendingTagListResult;
use wire::gateway::memory::TagAddResult;
use wire::gateway::memory::TagCountsResult;
use wire::gateway::memory::TagDecisionResult;
use wire::gateway::memory::TagListResult;
use wire::gateway::memory::TagLookupResult;
use wire::gateway::memory::TagState;
use wire::gateway::memory::TagTaxonomyDecideRequest;
use wire::gateway::memory::TagTaxonomyListResult;
use wire::gateway::memory::TagTaxonomyMergeRequest;
use wire::gateway::memory::TagTaxonomyMergeResult;
use wire::gateway::memory::TagTaxonomyUpsertRequest;
use wire::gateway::memory::TagTaxonomyUpsertResult;
use wire::memory::MemoryEntry;
use wire::message::Role;

const SESSION: &str = "claude-code:11111111-2222-4333-8444-555555555555";

fn recorded(method: &str, target: &str, body: Vec<u8>) -> Request {
    Request {
        method: method.to_string(),
        target: target.to_string(),
        headers: vec![("Host".to_string(), "127.0.0.1:5174".to_string())],
        body,
        recipe: None,
        via: None,
    }
}

fn post<T: serde::Serialize>(
    app: &axum::Router,
    path: &str,
    value: &T,
) -> Result<testkit::Response, String> {
    let body = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    block_on(answer(app.clone(), recorded("POST", path, body)))
}

fn seeded() -> Result<(Arc<testkit::FakeMemoryStore>, axum::Router), String> {
    let (gateway, store) = common::gateway();
    let batch = vec![
        entry("m1", "hello world", "2026-10-10T12:00:00Z"),
        entry("m2", "goodbye", "2026-10-10T13:00:00Z"),
    ];
    block_on(store.append(batch)).map_err(|error| error.to_string())?;
    Ok((store, gateway.router(false)))
}

fn entry(id: &str, content: &str, created_at: &str) -> MemoryEntry {
    MemoryEntry {
        id: Some(id.to_owned()),
        session_id: SESSION.to_owned(),
        agent: "rivet-claude".to_owned(),
        channel: "claude-code".to_owned(),
        role: Role::User,
        content: content.to_owned(),
        tool_name: None,
        tool_args: None,
        tool_result: None,
        metadata: None,
        created_at: chrono::DateTime::parse_from_rfc3339(created_at)
            .ok()
            .map(|parsed| parsed.with_timezone(&chrono::Utc)),
    }
}

fn error_of(response: &testkit::Response) -> GatewayError {
    serde_json::from_slice(&response.body)
        .unwrap_or_else(|_| GatewayError::new(String::from_utf8_lossy(&response.body).into_owned()))
}

fn tool_text(response: &testkit::Response) -> String {
    let parsed: MemoryToolResponse =
        serde_json::from_slice(&response.body).unwrap_or_else(|_| MemoryToolResponse {
            ok: false,
            result: wire::message::ToolResult::Text(String::new()),
        });
    let wire::message::ToolResult::Text(text) = parsed.result else {
        return String::new();
    };
    text
}

fn full_browse_filter() -> kernel::memory::BrowseFilter {
    kernel::memory::BrowseFilter {
        conversation_id: None,
        agent: None,
        role: None,
        tool_name: None,
        tag: None,
        window: None,
        since: None,
        before: None,
        include_tools: false,
        order: wire::gateway::memory::MemoryBrowseOrder::Desc,
        limit: Limit::clamped(None, 50, 1..=200),
    }
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "only the search arm carries a query; the fake records sixteen request kinds"
)]
fn recorded_searches(requests: &[testkit::MemoryRequest]) -> Vec<&kernel::memory::SearchQuery> {
    requests
        .iter()
        .filter_map(|request| match request {
            testkit::MemoryRequest::Search(query) => Some(query),
            _ => None,
        })
        .collect()
}

#[expect(
    clippy::wildcard_enum_match_arm,
    reason = "only the browse arm carries a filter; the fake records sixteen request kinds"
)]
fn recorded_browses(requests: &[testkit::MemoryRequest]) -> Vec<&kernel::memory::BrowseFilter> {
    requests
        .iter()
        .filter_map(|request| match request {
            testkit::MemoryRequest::Browse(filter) => Some(filter),
            _ => None,
        })
        .collect()
}

fn search_rejections(app: &axum::Router) -> Result<(), String> {
    let call = |method: &str, target: &str| {
        block_on(answer(app.clone(), recorded(method, target, Vec::new())))
    };
    let response = call("GET", "/api/memory/search")?;
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "q required");

    let response = call("GET", "/api/memory/search?q=x&scope=bogus")?;
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "invalid scope");

    let response = call("GET", "/api/memory/search?q=x&limit=abc")?;
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "invalid limit");

    let response = call("GET", "/api/memory/search?q=x&limit=0")?;
    assert_eq!(response.status, 200);

    let response = call("GET", "/api/memory/search?q=x&tag=nokey")?;
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "tag must be key:value");

    let response = call("POST", "/api/memory/search")?;
    assert_eq!(response.status, 405);
    assert_eq!(error_of(&response).error, "method not allowed");
    Ok(())
}

#[test]
fn search_requires_q_and_maps_hits() {
    let (_store, app) = seeded().expect("seed store");
    search_rejections(&app).expect("search rejections hold");

    let response = block_on(answer(
        app,
        recorded("GET", "/api/memory/search?q=hello", Vec::new()),
    ))
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: MemorySearchResponse =
        serde_json::from_slice(&response.body).expect("search response");
    assert_eq!(parsed.query, "hello");
    assert_eq!(parsed.scope, wire::gateway::memory::MemoryScope::Both);
    assert_eq!(parsed.results.len(), 1);
    let hit = parsed.results.first().expect("hit present");
    assert_eq!(hit.id, "m1");
    assert_eq!(hit.source, wire::gateway::memory::MemoryHitSource::Message);
    assert_eq!(hit.content, "hello world");
    assert_eq!(hit.role, Some(Role::User));
    assert_eq!(hit.agent.as_deref(), Some("rivet-claude"));
}

#[test]
fn search_scope_and_filters_reach_the_port() {
    let (store, app) = seeded().expect("seed store");
    let first = block_on(answer(
        app.clone(),
        recorded(
            "GET",
            "/api/memory/search?q=hello&scope=summaries",
            Vec::new(),
        ),
    ))
    .expect("in-process answer");
    assert_eq!(first.status, 200);
    let second = block_on(answer(
        app,
        recorded(
            "GET",
            "/api/memory/search?q=hello&agent=rivet-claude&tag=project:acmekit&limit=7",
            Vec::new(),
        ),
    ))
    .expect("in-process answer");
    assert_eq!(second.status, 200);
    let requests = block_on(store.requests());
    let searches = recorded_searches(&requests);
    assert_eq!(searches.len(), 2);
    let first_query = searches.first().expect("first search");
    assert_eq!(
        first_query.scope,
        wire::gateway::memory::MemoryScope::Summaries
    );
    let second_query = searches.get(1).expect("second search");
    assert_eq!(second_query.scope, wire::gateway::memory::MemoryScope::Both);
    assert_eq!(second_query.agent.as_deref(), Some("rivet-claude"));
    assert_eq!(
        second_query.tag,
        Some(kernel::memory::TagLiteral {
            key: "project".to_owned(),
            value: "acmekit".to_owned(),
        })
    );
    assert_eq!(second_query.limit, Limit::clamped(Some(7), 20, 1..=50));
}

#[test]
fn browse_returns_messages() {
    let (store, app) = seeded().expect("seed store");
    let response = block_on(answer(
        app,
        recorded("GET", "/api/memory/browse", Vec::new()),
    ))
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: MemoryBrowseResponse =
        serde_json::from_slice(&response.body).expect("browse response");
    assert_eq!(parsed.messages.len(), 2);
    let message = parsed.messages.first().expect("message present");
    assert_eq!(message.id, "m1");
    assert_eq!(message.role, Role::User);
    assert_eq!(message.agent.as_deref(), Some("rivet-claude"));
    assert_eq!(message.session_id.as_deref(), Some(SESSION));
    let requests = block_on(store.requests());
    assert!(
        requests
            .iter()
            .any(|request| { *request == testkit::MemoryRequest::Browse(full_browse_filter()) })
    );
}

#[test]
fn browse_rejects_bad_filters() {
    let (store, app) = seeded().expect("seed store");
    let call = |target: &str| block_on(answer(app.clone(), recorded("GET", target, Vec::new())));

    let response = call("/api/memory/browse?tag=x").expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "tag must be key:value");

    let response = call("/api/memory/browse?window=fortnight").expect("in-process answer");
    assert_eq!(response.status, 400);
    let text = error_of(&response).error;
    assert!(text.starts_with("Unknown window=\"fortnight\""), "{text}");
    assert!(text.contains("last_7d"), "{text}");

    let response = call("/api/memory/browse?window=Last%207d").expect("in-process answer");
    assert_eq!(response.status, 200);

    let response =
        call("/api/memory/browse?role=user&tool_name=Bash&tag=project:acme&window=today&limit=11")
            .expect("in-process answer");
    assert_eq!(response.status, 200);
    let requests = block_on(store.requests());
    let filter = recorded_browses(&requests)
        .into_iter()
        .next_back()
        .expect("browse reached the port");
    assert_eq!(filter.role, Some(Role::User));
    assert_eq!(filter.tool_name.as_deref(), Some("Bash"));
    assert_eq!(
        filter.tag,
        Some(kernel::memory::TagLiteral {
            key: "project".to_owned(),
            value: "acme".to_owned(),
        })
    );
    assert_eq!(
        filter.window,
        Some(wire::gateway::memory::BrowseWindow::Today)
    );
    assert_eq!(filter.limit, Limit::clamped(Some(11), 50, 1..=200));
}

#[test]
fn stats_passthrough_the_store_shape() {
    let (_store, app) = seeded().expect("seed store");
    let response = block_on(answer(
        app,
        recorded("GET", "/api/memory/stats", Vec::new()),
    ))
    .expect("answer");
    assert_eq!(response.status, 200);
    let parsed: wire::gateway::memory::MemoryStatsResponse =
        serde_json::from_slice(&response.body).expect("stats response");
    assert_eq!(parsed.conversations, 1);
    assert_eq!(parsed.messages, 2);
    assert_eq!(parsed.tool_calls, 0);
    assert_eq!(parsed.embed_queue_depth, 0);
    assert_eq!(parsed.top_tools, Vec::new());
    assert_eq!(parsed.recent_sessions, Vec::new());
}

#[test]
fn get_full_tool_reports_the_documented_strings() {
    let (_store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tool/memory_get_full",
        &MemoryGetFullToolArgs { id: None },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    assert_eq!(
        tool_text(&response),
        "memory_get_full: id is required (string)."
    );

    let response = post(
        &app,
        "/api/memory/tool/memory_get_full",
        &MemoryGetFullToolArgs {
            id: Some("zz".to_owned()),
        },
    )
    .expect("in-process answer");
    assert_eq!(tool_text(&response), "No message with id zz.");

    let response = post(
        &app,
        "/api/memory/tool/memory_get_full",
        &MemoryGetFullToolArgs {
            id: Some("m1".to_owned()),
        },
    )
    .expect("in-process answer");
    let text = tool_text(&response);
    assert!(
        text.starts_with("(row was not truncated — stored payload is complete)"),
        "{text}"
    );
    assert!(text.contains("hello world"), "{text}");
}

#[test]
fn search_tool_answers_json_text_and_reports_failures() {
    let (_store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tool/memory_search",
        &MemorySearchToolArgs {
            query: "hello".to_owned(),
            mode: None,
            scope: None,
            limit: Some(5),
            agent: None,
            tag: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let inner: MemorySearchResponse =
        serde_json::from_str(&tool_text(&response)).expect("tool result is json text");
    assert_eq!(inner.query, "hello");
    assert_eq!(inner.results.len(), 1);

    let response = post(
        &app,
        "/api/memory/tool/memory_search",
        &MemorySearchToolArgs {
            query: "hello".to_owned(),
            mode: Some("bogus".to_owned()),
            scope: None,
            limit: None,
            agent: None,
            tag: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(
        tool_text(&response),
        "Search failed: Unknown search mode: bogus"
    );

    let response = post(
        &app,
        "/api/memory/tool/memory_search",
        &MemorySearchToolArgs {
            query: "hello".to_owned(),
            mode: None,
            scope: None,
            limit: None,
            agent: None,
            tag: Some("nokey".to_owned()),
        },
    )
    .expect("in-process answer");
    assert_eq!(
        tool_text(&response),
        "Search failed: tag must be key:value (got \"nokey\")"
    );
}

#[test]
fn stats_tool_answers_json_text() {
    let (_store, app) = seeded().expect("seed store");
    let empty = serde_json::value::RawValue::from_string("{}".to_owned()).expect("raw");
    let response = post(&app, "/api/memory/tool/memory_stats", &empty).expect("in-process answer");
    assert_eq!(response.status, 200);
    let inner: wire::gateway::memory::MemoryStatsResponse =
        serde_json::from_str(&tool_text(&response)).expect("stats tool result is json text");
    assert_eq!(inner.messages, 2);
}

#[test]
fn browse_tool_validates_args() {
    let (_store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tool/memory_browse",
        &MemoryBrowseToolArgs {
            conversation_id: None,
            include_tools: None,
            since: None,
            before: None,
            window: Some("fortnight".to_owned()),
            agent: None,
            tag: None,
            limit: None,
            order: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let text = tool_text(&response);
    assert!(
        text.starts_with("Browse failed: Unknown window=\"fortnight\""),
        "{text}"
    );

    let response = post(
        &app,
        "/api/memory/tool/memory_browse",
        &MemoryBrowseToolArgs {
            conversation_id: None,
            include_tools: None,
            since: None,
            before: None,
            window: None,
            agent: None,
            tag: Some("nokey".to_owned()),
            limit: None,
            order: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(
        tool_text(&response),
        "Browse failed: tag must be key:value (got \"nokey\")"
    );

    let response = post(
        &app,
        "/api/memory/tool/memory_browse",
        &MemoryBrowseToolArgs {
            conversation_id: Some("zz".to_owned()),
            include_tools: None,
            since: None,
            before: None,
            window: None,
            agent: None,
            tag: None,
            limit: None,
            order: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(
        tool_text(&response),
        "Browse failed: conversation_id must be a uuid"
    );
}

#[test]
fn browse_tool_carries_filters_to_the_port() {
    let (store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tool/memory_browse",
        &MemoryBrowseToolArgs {
            conversation_id: Some("00000000-0000-4000-8000-000000000009".to_owned()),
            include_tools: Some(true),
            since: None,
            before: None,
            window: Some("last_7d".to_owned()),
            agent: Some("rivet-claude".to_owned()),
            tag: Some("project:acmekit".to_owned()),
            limit: Some(3),
            order: Some(wire::gateway::memory::MemoryBrowseOrder::Asc),
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: MemoryBrowseResponse =
        serde_json::from_str(&tool_text(&response)).expect("browse json");
    assert_eq!(parsed.messages.len(), 2);
    let requests = block_on(store.requests());
    let filter = recorded_browses(&requests)
        .into_iter()
        .next_back()
        .expect("browse reached the port");
    assert_eq!(
        filter.tag,
        Some(kernel::memory::TagLiteral {
            key: "project".to_owned(),
            value: "acmekit".to_owned(),
        })
    );
    assert_eq!(
        filter.window,
        Some(wire::gateway::memory::BrowseWindow::Last7d)
    );
    assert!(filter.include_tools);
    assert_eq!(filter.order, wire::gateway::memory::MemoryBrowseOrder::Asc);
    assert_eq!(
        filter.conversation_id.map(|id| id.to_string()).as_deref(),
        Some("00000000-0000-4000-8000-000000000009")
    );
    assert_eq!(filter.agent.as_deref(), Some("rivet-claude"));
}

#[test]
fn append_tool_writes_through_the_port() {
    let (store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tool/memory_append",
        &wire::gateway::memory::MemoryAppendToolArgs {
            session_id: "s-app".to_owned(),
            content: "appended".to_owned(),
            role: Role::User,
            tool_name: None,
            tool_args: None,
            tool_result: None,
            event_id: None,
            agent: Some("a".to_owned()),
            persona: None,
            source: None,
            channel: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let inner: wire::gateway::OkReply =
        serde_json::from_str(&tool_text(&response)).expect("ok reply");
    assert!(inner.ok);
    let all = block_on(store.browse(full_browse_filter())).unwrap_or_default();
    assert_eq!(all.len(), 3);
}

#[test]
fn ingest_session_tool_writes_through_the_port() {
    let (store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tool/memory_ingest_session",
        &wire::gateway::memory::MemoryIngestSessionToolArgs {
            session_id: "s-ingest".to_owned(),
            messages: vec![
                wire::gateway::memory::MemoryIngestMessage {
                    role: Role::User,
                    content: "one".to_owned(),
                    created_at: None,
                    tool_calls: None,
                },
                wire::gateway::memory::MemoryIngestMessage {
                    role: Role::Assistant,
                    content: "two".to_owned(),
                    created_at: None,
                    tool_calls: None,
                },
            ],
            agent: None,
            persona: None,
            source: None,
            channel: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let inner: wire::capture::CaptureResult =
        serde_json::from_str(&tool_text(&response)).expect("capture result");
    assert_eq!(inner.inserted, 2);
    let all = block_on(store.browse(full_browse_filter())).unwrap_or_default();
    assert_eq!(all.len(), 4);
}

#[test]
fn unknown_memory_tools_are_404() {
    let (_store, app) = seeded().expect("seed store");
    let raw_tool = |name: &str, raw: &[u8]| {
        block_on(answer(
            app.clone(),
            recorded("POST", &format!("/api/memory/tool/{name}"), raw.to_vec()),
        ))
    };
    let response = raw_tool("memory_tags", b"{}").expect("in-process answer");
    assert_eq!(response.status, 404);
    assert_eq!(error_of(&response).error, "unknown memory tool");

    let response = raw_tool("not_a_tool", b"{}").expect("in-process answer");
    assert_eq!(response.status, 404);
    assert_eq!(error_of(&response).error, "unknown memory tool");

    let response = block_on(answer(
        app.clone(),
        recorded("GET", "/api/memory/tool/memory_stats", Vec::new()),
    ))
    .expect("in-process answer");
    assert_eq!(response.status, 405);
}

#[test]
fn tool_bodies_are_validated() {
    let (_store, app) = seeded().expect("seed store");
    let raw_tool = |raw: &[u8]| {
        block_on(answer(
            app.clone(),
            recorded("POST", "/api/memory/tool/memory_search", raw.to_vec()),
        ))
    };
    let response = raw_tool(br#"{"query": 5}"#).expect("in-process answer");
    assert_eq!(response.status, 400);

    let response = raw_tool(b"[]").expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "body must be a JSON object");

    let response = raw_tool(b"").expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "invalid JSON");
}

#[test]
fn tags_read_routes_cover_the_port_surface() {
    let (_store, app) = seeded().expect("seed store");
    let call = |method: &str, path: &str| {
        block_on(answer(
            app.clone(),
            recorded(method, &format!("/api/memory/tags{path}"), Vec::new()),
        ))
    };
    let response = call("GET", "").expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: TagListResult = serde_json::from_slice(&response.body).expect("tags response");
    assert_eq!(parsed.tags, Vec::new());

    let response = call("GET", "/pending").expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: PendingTagListResult =
        serde_json::from_slice(&response.body).expect("pending response");
    assert_eq!(parsed.tags, Vec::new());

    let response = call("GET", "/counts").expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: TagCountsResult = serde_json::from_slice(&response.body).expect("counts response");
    assert_eq!(parsed.counts, Vec::new());

    let response = call("GET", "/taxonomy").expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: TagTaxonomyListResult =
        serde_json::from_slice(&response.body).expect("taxonomy response");
    assert_eq!(parsed.entries, Vec::new());

    let response = call("GET", "/decide").expect("in-process answer");
    assert_eq!(response.status, 405);

    let response = call("GET", "/nope").expect("in-process answer");
    assert_eq!(response.status, 404);
    assert_eq!(error_of(&response).error, "unknown tags resource");
}

#[test]
fn tags_read_routes_reject_bad_params() {
    let (_store, app) = seeded().expect("seed store");
    let call = |target: &str| block_on(answer(app.clone(), recorded("GET", target, Vec::new())));
    let response = call("/api/memory/tags?entity_type=bogus").expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "bad entity_type");

    let response = call("/api/memory/tags?entity_id=zz").expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "bad entity_id");

    let response = call("/api/memory/tags?state=bogus").expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "bad state");

    let response = call("/api/memory/tags?limit=abc").expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "invalid limit");
}

#[test]
fn tags_decide_validates() {
    let (_store, app) = seeded().expect("seed store");
    let decide = |request: &MemoryTagsDecideRequest| post(&app, "/api/memory/tags/decide", request);
    let response = decide(&MemoryTagsDecideRequest {
        ids: Some(Vec::new()),
        state: Some(TagState::Accepted),
        decided_by: None,
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "ids required");

    let response = decide(&MemoryTagsDecideRequest {
        ids: Some(vec!["t1".to_owned()]),
        state: Some(TagState::Suggested),
        decided_by: None,
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(
        error_of(&response).error,
        "state must be accepted or rejected"
    );

    let response = decide(&MemoryTagsDecideRequest {
        ids: Some(vec!["t1".to_owned()]),
        state: Some(TagState::Accepted),
        decided_by: None,
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "ids must be uuids");

    let response = decide(&MemoryTagsDecideRequest {
        ids: Some(vec![
            "00000000-0000-4000-8000-000000000000".to_owned();
            1001
        ]),
        state: Some(TagState::Accepted),
        decided_by: None,
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "at most 1000 ids");
}

#[test]
fn tags_decide_reports_changed() {
    let (_store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tags/decide",
        &MemoryTagsDecideRequest {
            ids: Some(vec![
                "00000000-0000-4000-8000-000000000001".to_owned(),
                "00000000-0000-4000-8000-000000000002".to_owned(),
            ]),
            state: Some(TagState::Rejected),
            decided_by: Some("owner".to_owned()),
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: TagDecisionResult =
        serde_json::from_slice(&response.body).expect("decide response");
    assert_eq!(parsed.changed, 0);
}

#[test]
fn tags_add_validates() {
    let (_store, app) = seeded().expect("seed store");
    let add = |request: &MemoryTagsAddRequest| post(&app, "/api/memory/tags/add", request);
    let response = add(&MemoryTagsAddRequest {
        entity_type: None,
        entity_id: Some("00000000-0000-4000-8000-000000000003".to_owned()),
        session_key: None,
        agent: None,
        tag: Some("project:acmekit".to_owned()),
        key: None,
        value: None,
        display: None,
        reason: None,
        decided_by: None,
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(
        error_of(&response).error,
        "entity_type and entity_id (or session_key) required"
    );

    let response = add(&MemoryTagsAddRequest {
        entity_type: Some("conversation".to_owned()),
        entity_id: Some("zz".to_owned()),
        session_key: None,
        agent: None,
        tag: Some("project:acmekit".to_owned()),
        key: None,
        value: None,
        display: None,
        reason: None,
        decided_by: None,
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "bad entity_id");

    let response = add(&MemoryTagsAddRequest {
        entity_type: Some("widget".to_owned()),
        entity_id: Some("00000000-0000-4000-8000-000000000003".to_owned()),
        session_key: None,
        agent: None,
        tag: Some("project:acmekit".to_owned()),
        key: None,
        value: None,
        display: None,
        reason: None,
        decided_by: None,
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "bad entity_type");

    let response = add(&MemoryTagsAddRequest {
        entity_type: Some("conversation".to_owned()),
        entity_id: None,
        session_key: Some(SESSION.to_owned()),
        agent: None,
        tag: Some("nokey".to_owned()),
        key: None,
        value: None,
        display: None,
        reason: None,
        decided_by: None,
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(
        error_of(&response).error,
        "invalid tag literal \"nokey\" (want key:value)"
    );
}

#[test]
fn tags_add_returns_the_tag() {
    let (_store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tags/add",
        &MemoryTagsAddRequest {
            entity_type: Some("conversation".to_owned()),
            entity_id: Some("00000000-0000-4000-8000-000000000003".to_owned()),
            session_key: None,
            agent: None,
            tag: Some("project:acmekit".to_owned()),
            key: None,
            value: None,
            display: Some("AcmeKit".to_owned()),
            reason: None,
            decided_by: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: TagAddResult = serde_json::from_slice(&response.body).expect("add response");
    assert_eq!(parsed.tag.key, "project");
    assert_eq!(parsed.tag.value, "acmekit");
    assert_eq!(parsed.tag.state, TagState::Accepted);
}

#[test]
fn tags_lookup_validates() {
    let (_store, app) = seeded().expect("seed store");
    let lookup = |request: &MemoryTagsLookupRequest| post(&app, "/api/memory/tags/lookup", request);
    let response = lookup(&MemoryTagsLookupRequest {
        session_keys: Some(Vec::new()),
        states: None,
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "session_keys required");

    let response = lookup(&MemoryTagsLookupRequest {
        session_keys: Some(vec!["k".to_owned(); 501]),
        states: None,
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "at most 500 session_keys");

    let response = lookup(&MemoryTagsLookupRequest {
        session_keys: Some(vec![SESSION.to_owned()]),
        states: Some(vec!["bogus".to_owned()]),
    })
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "bad state");
}

#[test]
fn tags_lookup_returns_sessions() {
    let (_store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tags/lookup",
        &MemoryTagsLookupRequest {
            session_keys: Some(vec![SESSION.to_owned()]),
            states: Some(vec!["suggested".to_owned()]),
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: TagLookupResult = serde_json::from_slice(&response.body).expect("lookup response");
    assert_eq!(parsed.sessions.len(), 0);
}

#[test]
fn tags_taxonomy_upsert_validates_and_returns_the_entry() {
    let (_store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tags/taxonomy",
        &TagTaxonomyUpsertRequest {
            key: None,
            value: Some("acmekit".to_owned()),
            display: None,
            parent_value: None,
            aliases: None,
            reason: None,
            decided_by: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "key and value required");

    let response = post(
        &app,
        "/api/memory/tags/taxonomy",
        &TagTaxonomyUpsertRequest {
            key: Some("project".to_owned()),
            value: Some("acmekit".to_owned()),
            display: Some("AcmeKit".to_owned()),
            parent_value: None,
            aliases: Some(vec!["acme".to_owned()]),
            reason: None,
            decided_by: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: TagTaxonomyUpsertResult =
        serde_json::from_slice(&response.body).expect("upsert response");
    assert_eq!(parsed.entry.key, "project");
    assert_eq!(parsed.entry.value, "acmekit");
}

#[test]
fn tags_taxonomy_decide_validates_and_reports_changed() {
    let (_store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tags/taxonomy/decide",
        &TagTaxonomyDecideRequest {
            entries: None,
            state: Some(TagState::Accepted),
            decided_by: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "entries required");

    let response = post(
        &app,
        "/api/memory/tags/taxonomy/decide",
        &TagTaxonomyDecideRequest {
            entries: Some(vec![wire::gateway::memory::TagEntryPair {
                key: "project".to_owned(),
                value: "acmekit".to_owned(),
            }]),
            state: Some(TagState::Suggested),
            decided_by: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(
        error_of(&response).error,
        "state must be accepted or rejected"
    );

    let response = post(
        &app,
        "/api/memory/tags/taxonomy/decide",
        &TagTaxonomyDecideRequest {
            entries: Some(vec![wire::gateway::memory::TagEntryPair {
                key: "project".to_owned(),
                value: "acmekit".to_owned(),
            }]),
            state: Some(TagState::Rejected),
            decided_by: None,
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: TagDecisionResult =
        serde_json::from_slice(&response.body).expect("decide response");
    assert_eq!(parsed.changed, 0);
}

#[test]
fn tags_taxonomy_merge_validates_and_merges() {
    let (_store, app) = seeded().expect("seed store");
    let response = post(
        &app,
        "/api/memory/tags/taxonomy/merge",
        &TagTaxonomyMergeRequest {
            key: Some("project".to_owned()),
            from: Some("acme".to_owned()),
            into: Some(String::new()),
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "key, from and into required");

    let response = post(
        &app,
        "/api/memory/tags/taxonomy/merge",
        &TagTaxonomyMergeRequest {
            key: Some("project".to_owned()),
            from: Some("acme".to_owned()),
            into: Some("acmekit".to_owned()),
        },
    )
    .expect("in-process answer");
    assert_eq!(response.status, 200);
    let parsed: TagTaxonomyMergeResult =
        serde_json::from_slice(&response.body).expect("merge response");
    assert_eq!(parsed.moved, 0);
    assert_eq!(parsed.dropped, 0);
    assert_eq!(parsed.into, "acmekit");
}

#[test]
fn tags_non_object_and_invalid_bodies() {
    let (_store, app) = seeded().expect("seed store");
    let raw = |path: &str, raw: &[u8]| {
        block_on(answer(
            app.clone(),
            recorded("POST", &format!("/api/memory/tags{path}"), raw.to_vec()),
        ))
    };
    let response = raw("/decide", b"[]").expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "body must be a JSON object");

    let response = raw("/decide", b"").expect("in-process answer");
    assert_eq!(response.status, 400);
    assert_eq!(error_of(&response).error, "invalid JSON");
}

#[test]
fn memory_fallback_and_health_shape() {
    let (_store, app) = seeded().expect("seed store");
    let call = |method: &str, target: &str| {
        block_on(answer(app.clone(), recorded(method, target, Vec::new())))
    };
    let response = call("GET", "/api/memory/health").expect("in-process answer");
    assert_eq!(response.status, 404);
    assert_eq!(error_of(&response).error, "unknown memory resource");

    let response = call("GET", "/api/memory").expect("in-process answer");
    assert_eq!(response.status, 404);
    assert_eq!(error_of(&response).error, "unknown memory resource");

    let response = call("GET", "/api/memory/bogus").expect("in-process answer");
    assert_eq!(response.status, 404);
    assert_eq!(error_of(&response).error, "unknown memory resource");
}

#[test]
fn store_failures_map_to_statuses() {
    let gateway = common::failing_gateway();
    let app = gateway.router(false);
    let response = block_on(answer(
        app,
        recorded("GET", "/api/memory/stats", Vec::new()),
    ))
    .expect("in-process answer");
    assert_eq!(response.status, 503);
    assert_eq!(error_of(&response).error, "memory store unavailable");
}
