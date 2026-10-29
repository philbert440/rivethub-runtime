use axum::http::StatusCode;
use kernel::topics::TopicId;
use kernel::topics::TopicPageStore as _;
use wire::gateway::topics::StartupContext;
use wire::gateway::topics::StartupKind;
use wire::gateway::topics::TopicRecord;

use super::Api;
use super::TestResult;

#[tokio::test]
async fn a_topic_previews_the_startup_memory_a_new_session_there_would_get() -> TestResult {
    let api = Api::with_chat().await?;
    let mesh: TopicRecord = api
        .json(
            "POST",
            "/api/topics",
            r#"{"name":"Mesh"}"#,
            StatusCode::CREATED,
        )
        .await?;
    let slug = "mesh".to_owned().try_into()?;
    let mut page = kernel::topics::TopicPage::empty(slug, "Mesh");
    page.summary = "Mesh certificates come from step-ca and renew weekly.".to_owned();
    api.store
        .save_topic_page(mesh.id.parse::<TopicId>()?, &page, chrono::Utc::now())
        .await?;
    let preview: StartupContext = api
        .json("GET", "/api/topics/mesh/startup", "", StatusCode::OK)
        .await?;
    let topic = preview.sections.first().ok_or("the page is a section")?;
    assert_eq!(topic.kind, StartupKind::Topic);
    assert!(topic.text.contains("renew weekly"), "{}", topic.text);
    assert_eq!(
        api.error("GET", "/api/topics/nowhere/startup", "").await?.0,
        StatusCode::NOT_FOUND
    );
    Ok(())
}
