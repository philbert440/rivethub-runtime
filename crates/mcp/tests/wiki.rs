#![cfg(test)]

use kernel::wiki::WikiFilter;
use mcp::Server;
use rmcp::model::CallToolResult;
use std::sync::Arc;
use testkit::{FakeMemoryStore, MemoryRequest};
use tokio_util::sync::CancellationToken;
use wire::gateway::wiki::{
    WikiCitation, WikiHistoryEntry, WikiPageResponse, WikiTopic, WikiTopicList,
};

pub mod support;

fn setup() -> (Server, Arc<FakeMemoryStore>) {
    let store = Arc::new(FakeMemoryStore::default());
    let server = Server::new(store.clone(), false, CancellationToken::new()).expect("server");
    (server, store)
}

fn page() -> WikiPageResponse {
    WikiPageResponse {
        slug: "acme".into(),
        title: "Acme 日本語".into(),
        aliases: vec!["AcmeKit".into(), "Acme's $(tool)".into()],
        tags: vec!["project".into()],
        entities: vec!["example.com".into()],
        current_state: "Ready 🦀".into(),
        article: Some("Article 日本語".into()),
        see_also: Some(vec!["related-topic".into()]),
        history: vec![
            WikiHistoryEntry {
                date: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).expect("date"),
                title: "Started".into(),
                body: "Older history".into(),
            },
            WikiHistoryEntry {
                date: chrono::NaiveDate::from_ymd_opt(2026, 10, 14).expect("date"),
                title: "Ready".into(),
                body: "Recent history".into(),
            },
        ],
        citations: vec![WikiCitation {
            summary_id: "00000000-0000-4000-8000-000000000001".into(),
            date: Some(
                chrono::DateTime::parse_from_rfc3339("2026-10-14T00:00:00Z")
                    .expect("instant")
                    .with_timezone(&chrono::Utc),
            ),
            kind: Some("summary".into()),
            note: Some("Decision 🦀".into()),
        }],
        markdown: "---\ntitle: Acme\n---\n# Acme\nReady 🦀\n".into(),
        sources: vec![],
        git_sha: None,
        last_verified: None,
        updated_at: chrono::DateTime::parse_from_rfc3339("2026-10-14T00:00:00Z")
            .expect("instant")
            .with_timezone(&chrono::Utc),
        related: None,
    }
}

fn text(result: &CallToolResult) -> &str {
    assert_ne!(result.is_error, Some(true), "{result:?}");
    assert_eq!(result.content.len(), 1);
    &result
        .content
        .first()
        .expect("content")
        .as_text()
        .expect("text result")
        .text
}

#[tokio::test]
async fn search_preserves_typed_results_and_validates_limits() {
    let (server, store) = setup();
    let page = page();
    let expected = WikiTopicList {
        topics: vec![WikiTopic {
            slug: page.slug.clone(),
            title: page.title.clone(),
            tags: page.tags.clone(),
            entities: page.entities.clone(),
            updated_at: page.updated_at,
            excerpt: page.current_state.clone(),
        }],
        total: 1,
    };
    store.insert_wiki_page(page).await;
    for (arguments, limit) in [
        (r#"{"query":"日本語"}"#, 5),
        (r#"{"query":"日本語","limit":1}"#, 1),
        (r#"{"query":"日本語","limit":20}"#, 20),
    ] {
        let result = support::call(&server, "wiki_search", arguments).await;
        let actual: WikiTopicList = serde_json::from_str(text(&result)).expect("typed topic list");
        assert_eq!(actual, expected);
        assert_eq!(
            store.requests().await.last(),
            Some(&MemoryRequest::WikiTopics(WikiFilter {
                query: Some("日本語".into()),
                tag: None,
                limit,
                offset: 0,
            }))
        );
    }
    let result = support::call(&server, "wiki_search", r#"{"query":"missing"}"#).await;
    assert_eq!(
        serde_json::from_str::<WikiTopicList>(text(&result)).expect("empty list"),
        WikiTopicList {
            topics: vec![],
            total: 0
        }
    );
}

#[tokio::test]
async fn read_uses_typed_sections_and_preserves_small_markdown() {
    let (server, store) = setup();
    let page = page();
    store.insert_wiki_page(page.clone()).await;
    let result = support::call(&server, "wiki_read", r#"{"slug":"acme"}"#).await;
    assert_eq!(text(&result), page.markdown);
    for (section, expected) in [
        ("full", page.markdown.as_str()),
        ("summary", "Ready 🦀"),
        ("article", "Article 日本語"),
        ("aliases", "AcmeKit\nAcme's $(tool)"),
        (
            "history",
            "### 2026-10-14 — Ready\nRecent history\n\n### 2026-10-01 — Started\nOlder history",
        ),
        (
            "citations",
            "00000000-0000-4000-8000-000000000001 — 2026-10-14T00:00:00.000Z — summary — Decision 🦀",
        ),
    ] {
        let result = support::call(
            &server,
            "wiki_read",
            &format!(r#"{{"slug":"acme","section":"{section}"}}"#),
        )
        .await;
        assert_eq!(text(&result), expected);
    }
    assert_eq!(
        store.requests().await,
        vec![MemoryRequest::WikiPage("acme".into()); 7]
    );
}

#[tokio::test]
async fn oversized_views_and_sections_count_unicode_characters() {
    let (server, store) = setup();
    let mut page = page();
    page.markdown = "🦀".repeat(24_000);
    store.insert_wiki_page(page.clone()).await;
    let result = support::call(&server, "wiki_read", r#"{"slug":"acme","section":"full"}"#).await;
    assert_eq!(text(&result), page.markdown);
    page.markdown.push('🦀');
    store.insert_wiki_page(page.clone()).await;
    let result = support::call(&server, "wiki_read", r#"{"slug":"acme"}"#).await;
    let view = text(&result);
    assert!(view.contains("## Summary\nReady 🦀"));
    assert!(view.contains("## Article\nArticle 日本語"));
    assert!(view.contains("related-topic"));
    assert!(view.contains("Recent history"));
    assert!(!view.contains("AcmeKit"));
    assert!(view.chars().count() <= 24_000);
    assert_eq!(
        support::call(&server, "wiki_read", r#"{"slug":"acme","section":"full"}"#)
            .await
            .is_error,
        Some(true)
    );
    page.title = "🦀".repeat(30_000);
    page.current_state.clone_from(&page.title);
    page.article = Some(page.title.clone());
    page.aliases = vec![page.title.clone()];
    page.see_also = Some(vec![page.title.clone()]);
    page.history
        .get_mut(1)
        .expect("recent history")
        .body
        .clone_from(&page.title);
    page.citations.first_mut().expect("citation").note = Some(page.title.clone());
    store.insert_wiki_page(page).await;
    for section in ["summary", "article", "history", "aliases", "citations"] {
        let result = support::call(
            &server,
            "wiki_read",
            &format!(r#"{{"slug":"acme","section":"{section}"}}"#),
        )
        .await;
        assert_eq!(text(&result).chars().count(), 24_000);
        assert!(text(&result).contains("truncated"));
    }
    let result = support::call(&server, "wiki_read", r#"{"slug":"acme"}"#).await;
    assert!(text(&result).chars().count() <= 24_000);
    assert!(text(&result).contains("## Summary"));
    assert!(text(&result).contains("## Article"));
    assert!(text(&result).contains("## History"));
}

#[tokio::test]
async fn redirects_resolve_and_cycles_invalid_targets_and_missing_pages_fail() {
    let (server, store) = setup();
    store.insert_wiki_page(page()).await;
    store
        .insert_wiki_redirect("old".into(), "alias".into())
        .await;
    store
        .insert_wiki_redirect("alias".into(), "acme".into())
        .await;
    let result = support::call(&server, "wiki_read", r#"{"slug":"old"}"#).await;
    assert_eq!(text(&result), page().markdown);
    assert_eq!(
        store.requests().await,
        ["old", "alias", "acme"].map(|slug| MemoryRequest::WikiPage(slug.into()))
    );
    for target in [
        "../outside",
        "/absolute",
        "..",
        "bad\\path",
        "old",
        "missing",
    ] {
        store
            .insert_wiki_redirect("alias".into(), target.into())
            .await;
        assert_eq!(
            support::call(&server, "wiki_read", r#"{"slug":"old"}"#)
                .await
                .is_error,
            Some(true),
            "{target}"
        );
    }
    for index in 0..17 {
        store
            .insert_wiki_redirect(format!("hop-{index}"), format!("hop-{}", index + 1))
            .await;
    }
    assert_eq!(
        support::call(&server, "wiki_read", r#"{"slug":"hop-0"}"#)
            .await
            .is_error,
        Some(true)
    );
}

#[tokio::test]
async fn invalid_arguments_never_reach_the_port() {
    let (server, store) = setup();
    for args in [
        "{}",
        r#"{"query":7}"#,
        r#"{"query":false}"#,
        r#"{"query":[]}"#,
        r#"{"query":null}"#,
        r#"{"query":"x","limit":0}"#,
        r#"{"query":"x","limit":21}"#,
        r#"{"query":"x","limit":1.5}"#,
        r#"{"query":"x","limit":"5"}"#,
        r#"{"query":"x","unknown":true}"#,
    ] {
        assert_eq!(
            support::call(&server, "wiki_search", args).await.is_error,
            Some(true),
            "{args}"
        );
    }
    for args in [
        "{}",
        r#"{"slug":7}"#,
        r#"{"slug":null}"#,
        r#"{"slug":""}"#,
        r#"{"slug":"../sibling"}"#,
        r#"{"slug":".."}"#,
        r#"{"slug":"/absolute"}"#,
        r#"{"slug":"a\\b"}"#,
        r#"{"slug":"a\u0000b"}"#,
        r#"{"slug":"acme","section":"invalid"}"#,
        r#"{"slug":"acme","section":true}"#,
        r#"{"slug":"acme","unknown":true}"#,
    ] {
        assert_eq!(
            support::call(&server, "wiki_read", args).await.is_error,
            Some(true),
            "{args}"
        );
    }
    assert_eq!(store.requests().await, Vec::<MemoryRequest>::new());
}

#[tokio::test]
async fn valid_literal_slugs_and_absent_sections_are_handled_explicitly() {
    let (server, store) = setup();
    let mut page = page();
    page.slug = "Acme's $(topic) 日本語".into();
    page.article = None;
    page.see_also = None;
    page.history.clear();
    page.citations.clear();
    page.aliases.clear();
    store.insert_wiki_page(page.clone()).await;
    let result = support::call(&server, "wiki_read", r#"{"slug":"Acme's $(topic) 日本語"}"#).await;
    assert_eq!(text(&result), page.markdown);
    assert_eq!(
        support::call(
            &server,
            "wiki_read",
            r#"{"slug":"Acme's $(topic) 日本語","section":"article"}"#
        )
        .await
        .is_error,
        Some(true)
    );
    for section in ["history", "aliases", "citations"] {
        let result = support::call(
            &server,
            "wiki_read",
            &format!(r#"{{"slug":"Acme's $(topic) 日本語","section":"{section}"}}"#),
        )
        .await;
        assert_eq!(text(&result), "");
    }
}

#[tokio::test]
async fn cancellation_prevents_wiki_port_calls() {
    let (server, store) = setup();
    server.shutdown();
    for (name, args) in [
        ("wiki_search", r#"{"query":"acme"}"#),
        ("wiki_read", r#"{"slug":"acme"}"#),
    ] {
        let result = support::call(&server, name, args).await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result
                .content
                .first()
                .expect("content")
                .as_text()
                .expect("error text")
                .text,
            "operation cancelled"
        );
    }
    assert_eq!(store.requests().await, Vec::<MemoryRequest>::new());
}

#[tokio::test]
async fn store_failures_are_tool_errors_and_search_totals_are_not_page_lengths() {
    let (server, store) = setup();
    for (name, args) in [
        ("wiki_search", r#"{"query":"acme"}"#),
        ("wiki_read", r#"{"slug":"acme"}"#),
    ] {
        store
            .fail_next_wiki_request(kernel::memory::MemoryStoreError::InvalidRequest {
                message: "wiki store rejected the request".into(),
            })
            .await;
        let result = support::call(&server, name, args).await;
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result
                .content
                .first()
                .expect("content")
                .as_text()
                .expect("error text")
                .text,
            "wiki store rejected the request"
        );
    }
    store.insert_wiki_page(page()).await;
    let mut second = page();
    second.slug = "acme-next".into();
    store.insert_wiki_page(second).await;
    let result = support::call(&server, "wiki_search", r#"{"query":"acme","limit":1}"#).await;
    let result: WikiTopicList = serde_json::from_str(text(&result)).expect("topic list");
    assert_eq!(result.total, 2);
    assert_eq!(result.topics.len(), 1);
    assert_eq!(result.topics.first().expect("topic").slug, "acme");
}
