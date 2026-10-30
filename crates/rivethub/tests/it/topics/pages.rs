use axum::http::StatusCode;
use wire::gateway::topics::TopicKind;
use wire::gateway::topics::TopicList;
use wire::gateway::topics::TopicPage;
use wire::gateway::topics::TopicRecord;

use super::Api;
use super::TestResult;

#[tokio::test]
async fn editing_an_entity_the_wiki_has_not_written_writes_its_page() -> TestResult {
    let api = Api::new(true)?;
    let types: TopicList = api
        .json("GET", "/api/topics?kind=type", "", StatusCode::OK)
        .await?;
    let hosts = types
        .topics
        .into_iter()
        .map(|listed| listed.topic)
        .find(|topic| topic.name == "Hosts")
        .ok_or("the Hosts type is seeded")?;
    let entity: TopicRecord = api
        .json(
            "POST",
            "/api/topics",
            &format!(
                r#"{{"kind":"entity","name":"AcmeKit build box","parent":"{}"}}"#,
                hosts.id
            ),
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(entity.kind, TopicKind::Entity);
    let path = format!("/api/topics/{}/page", entity.id);
    let unwritten: TopicPage = api.json("GET", &path, "", StatusCode::OK).await?;
    assert_eq!(unwritten.page, None);
    let edited: TopicPage = api
        .json(
            "PUT",
            &path,
            r#"{"currentState":"Builds AcmeKit every night."}"#,
            StatusCode::OK,
        )
        .await?;
    let page = edited.page.ok_or("editing writes the page")?;
    assert_eq!(page.current_state, "Builds AcmeKit every night.");
    assert_eq!(page.topic_id.as_deref(), Some(entity.id.as_str()));
    let read: TopicPage = api.json("GET", &path, "", StatusCode::OK).await?;
    assert_eq!(read.page.map(|page| page.slug), Some(page.slug.clone()));
    assert_eq!(
        api.error(
            "PUT",
            &format!("/api/topics/{}/page", hosts.id),
            r#"{"article":"Machines."}"#
        )
        .await?,
        (StatusCode::CONFLICT, "A type has no page".to_owned())
    );
    assert_eq!(
        api.call(
            "PUT",
            &format!("/api/wiki/{}", page.slug),
            r#"{"article":"Moved."}"#
        )
        .await?
        .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    Ok(())
}
