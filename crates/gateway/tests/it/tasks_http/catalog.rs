use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use wire::gateway::catalog::CatalogAgentsResponse;
use wire::gateway::catalog::CatalogSheet;

use super::COMMAND_LIST_TIMEOUT;
use super::Catalog;
use super::Setup;
use super::call;
use super::json as parse;
use super::message;

#[test]
fn command_list_timeout_is_three_seconds() {
    assert_eq!(COMMAND_LIST_TIMEOUT, Duration::from_millis(3_000));
}

#[tokio::test]
async fn sheet_replaces_stale_commands() {
    let setup = Setup::new();
    let (status, bytes) = call(&setup.router(), "GET", "/api/catalog", None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let sheet: CatalogSheet = parse(&bytes);
    assert_eq!(sheet.node, "local");
    assert_eq!(sheet.tools, vec!["bash".to_owned()]);
    assert_eq!(sheet.skills.first().expect("skill").name, "review");
    let entry = sheet.executors.first().expect("executor");
    assert_eq!(entry.key, "chat-loop");
    assert_eq!(entry.implemented, Some(true));
    assert_eq!(entry.commands.first().expect("command").name, "real");
    let (status, bytes) = call(&setup.router(), "GET", "/api/catalog/agents", None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let agents: CatalogAgentsResponse = parse(&bytes);
    assert_eq!(agents.agents.len(), 1);
}

#[tokio::test]
async fn unknown_catalog_section_is_not_found() {
    let setup = Setup::new();
    let (status, bytes) = call(&setup.router(), "GET", "/api/catalog/nope", None).await;
    assert_eq!(status, 404, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "no catalog section \"nope\"");
    let (status, bytes) = call(&setup.router(), "POST", "/api/catalog", Some("{}")).await;
    assert_eq!(status, 405, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "method not allowed");
}

#[tokio::test]
async fn a_hung_command_list_becomes_empty() {
    let catalog = Arc::new(Catalog::steady());
    catalog.hang.store(true, Ordering::SeqCst);
    let mut setup = Setup::new();
    setup.catalog = Arc::clone(&catalog);
    setup.timeout = Duration::from_millis(30);
    let (status, bytes) = call(&setup.router(), "GET", "/api/catalog", None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let sheet: CatalogSheet = parse(&bytes);
    let entry = sheet.executors.first().expect("executor");
    assert_eq!(entry.key, "chat-loop");
    assert_eq!(entry.commands, Vec::new());
    assert_eq!(entry.implemented, Some(true));
    assert_eq!(
        catalog.seen.lock().first().map(String::as_str),
        Some("chat-loop")
    );
}

#[tokio::test]
async fn an_empty_catalog_still_answers() {
    let mut setup = Setup::new();
    setup.catalog = Arc::new(Catalog::empty());
    let (status, bytes) = call(&setup.router(), "GET", "/api/catalog", None).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&bytes));
    let sheet: CatalogSheet = parse(&bytes);
    assert_eq!(sheet.node, "local");
    assert_eq!(sheet.agents, Vec::new());
    assert_eq!(sheet.executors, Vec::new());
    assert_eq!(sheet.tools, Vec::<String>::new());
    assert_eq!(sheet.skills, Vec::new());
}

#[tokio::test]
async fn a_command_list_error_is_the_store_display() {
    let catalog = Arc::new(Catalog::steady());
    catalog.fail.store(true, Ordering::SeqCst);
    let mut setup = Setup::new();
    setup.catalog = catalog;
    let (status, bytes) = call(&setup.router(), "GET", "/api/catalog", None).await;
    assert_eq!(status, 500, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(message(&bytes), "graphile worker queue is absent");
}
