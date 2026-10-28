use axum::http::StatusCode;
use wire::gateway::topics::DecisionStatus;
use wire::gateway::topics::DecisionSuperseded;
use wire::gateway::topics::TopicDecision;
use wire::gateway::topics::TopicDecisionList;
use wire::gateway::topics::TopicRecord;

use super::Api;
use super::TestResult;

#[tokio::test]
async fn decisions_are_added_edited_and_superseded() -> TestResult {
    let api = Api::new(true)?;
    let _mesh: TopicRecord = api
        .json(
            "POST",
            "/api/topics",
            r#"{"name":"Mesh"}"#,
            StatusCode::CREATED,
        )
        .await?;
    let added: TopicDecision = api
        .json(
            "POST",
            "/api/topics/mesh/decisions",
            r#"{"text":"Use step-ca"}"#,
            StatusCode::CREATED,
        )
        .await?;
    assert_eq!(
        (added.text.as_str(), added.status, added.by.as_str()),
        ("Use step-ca", DecisionStatus::Active, "owner")
    );
    let path = format!("/api/topics/mesh/decisions/{}", added.id);
    let edited: TopicDecision = api
        .json(
            "PATCH",
            &path,
            r#"{"text":"Use step-ca for mesh certificates"}"#,
            StatusCode::OK,
        )
        .await?;
    assert_eq!(edited.text, "Use step-ca for mesh certificates");
    let swapped: DecisionSuperseded = api
        .json(
            "POST",
            &format!("{path}/supersede"),
            r#"{"text":"Renew every 7 days"}"#,
            StatusCode::OK,
        )
        .await?;
    let replacement = swapped
        .replacement
        .ok_or("supersede with text adds a replacement")?;
    assert_eq!(swapped.superseded.status, DecisionStatus::Superseded);
    assert_eq!(
        swapped.superseded.superseded_by,
        Some(replacement.id.clone())
    );
    let listed: TopicDecisionList = api
        .json("GET", "/api/topics/mesh/decisions", "", StatusCode::OK)
        .await?;
    assert_eq!(listed.decisions, [swapped.superseded, replacement]);
    assert_eq!(
        api.error("PATCH", &path, r#"{"text":"again"}"#).await?.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        api.error("POST", "/api/topics/mesh/decisions", r#"{"text":" "}"#)
            .await?
            .0,
        StatusCode::BAD_REQUEST
    );
    Ok(())
}
