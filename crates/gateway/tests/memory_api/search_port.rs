use super::*;
use kernel::memory::{SearchMode, SearchOutcome};
use wire::gateway::memory::{BrowseWindow, MemoryDegraded, MemoryScope};

#[test]
fn search_routes_forward_every_argument_and_preserve_runtime_degradation() {
    let (store, app) = seeded().expect("store");
    let outcome = SearchOutcome {
        degraded: Some(MemoryDegraded {
            reason: "http 503".into(),
            effect: "Keyword / FTS ranking only — not meaning-based.".into(),
        }),
        synthesis: Some("A focused answer".into()),
        ..Default::default()
    };
    block_on(store.set_search_outcome(outcome.clone()));
    let args = MemorySearchToolArgs {
        query: "release".into(),
        mode: Some("vector".into()),
        scope: Some(MemoryScope::Both),
        limit: Some(3),
        agent: Some("acme".into()),
        tag: Some("project:acme".into()),
        since: Some("2026-10-12T00:00:00Z".parse().expect("since")),
        before: Some("2026-10-13T00:00:00Z".parse().expect("before")),
        window: Some(BrowseWindow::Today),
        expand: Some(true),
        synthesize: Some(true),
    };
    let tool = post(&app, "/api/memory/tool/memory_search", &args).expect("tool");
    let tool_result: MemorySearchResponse =
        serde_json::from_str(&tool_text(&tool)).expect("result");
    let http = block_on(answer(app, recorded("GET", "/api/memory/search?q=release&mode=vector&scope=both&limit=3&agent=acme&tag=project%3Aacme&since=2026-10-12T00%3A00%3A00Z&before=2026-10-13T00%3A00%3A00Z&window=today&expand=true&synthesize=true", Vec::new()))).expect("http");
    assert_eq!(http.status, 200);
    let http_result: MemorySearchResponse = serde_json::from_slice(&http.body).expect("result");
    assert_eq!(http_result, tool_result);
    assert_eq!(http_result.degraded, outcome.degraded);
    assert_eq!(http_result.synthesis, outcome.synthesis);
    let requests = block_on(store.requests());
    let searches = recorded_searches(&requests);
    let [query, second] = searches.as_slice() else {
        panic!("two searches")
    };
    assert_eq!(query, second);
    assert_eq!(query.text, args.query);
    assert_eq!(query.mode, SearchMode::Vector);
    assert_eq!(query.scope, MemoryScope::Both);
    assert_eq!(query.agent, args.agent);
    assert_eq!(
        query.tag.as_ref().map(|tag| tag.value.as_str()),
        Some("acme")
    );
    assert_eq!(query.limit.get(), 3);
    assert_eq!(query.since, args.since);
    assert_eq!(query.before, args.before);
    assert_eq!(query.window, args.window);
    assert_eq!(query.expand, args.expand);
    assert_eq!(query.synthesize, args.synthesize);
}

#[test]
fn search_routes_reject_wrong_types_before_the_port() {
    let (store, app) = seeded().expect("store");
    for target in [
        "/api/memory/search?q=x&expand=yes",
        "/api/memory/search?q=x&synthesize=1",
        "/api/memory/search?q=x&since=wrong",
        "/api/memory/search?q=x&window=wrong",
        "/api/memory/search?q=x&mode=wrong",
    ] {
        let response =
            block_on(answer(app.clone(), recorded("GET", target, Vec::new()))).expect("http");
        assert_eq!(response.status, 400, "{target}");
    }
    for body in [
        r#"{"query":"x","since":2}"#,
        r#"{"query":"x","expand":"true"}"#,
        r#"{"query":"x","synthesize":1}"#,
    ] {
        let response = block_on(answer(
            app.clone(),
            recorded(
                "POST",
                "/api/memory/tool/memory_search",
                body.as_bytes().to_vec(),
            ),
        ))
        .expect("http");
        assert_eq!(response.status, 400);
    }
    assert_eq!(
        recorded_searches(&block_on(store.requests())),
        Vec::<&kernel::memory::SearchQuery>::new()
    );
}
