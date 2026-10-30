use axum::http::StatusCode;
use kernel::topics::Suggestion;
use kernel::topics::TopicName;
use kernel::topics::TopicStore as _;
use wire::gateway::topics::PlacedBy;
use wire::gateway::topics::PlacementReply;
use wire::gateway::topics::SessionHistoryList;
use wire::gateway::topics::SessionHistoryMark;
use wire::gateway::topics::SessionSuggestion;
use wire::gateway::topics::SessionSuggestionList;
use wire::gateway::topics::TopicFiling;
use wire::gateway::topics::TopicRecord;
use wire::gateway::topics::TopicSessionList;
use wire::session_id::SessionId;
use wire::session_id::encode_segment;

use super::Api;
use super::TestResult;

#[tokio::test]
async fn every_filing_and_suggestion_reads_in_one_request() -> TestResult {
    let api = Api::new(true)?;
    let release: TopicRecord = api
        .json(
            "POST",
            "/api/topics",
            r#"{"name":"Release"}"#,
            StatusCode::CREATED,
        )
        .await?;
    let filed: SessionId = "claude-code:00000000-0000-4000-8000-000000000051".parse()?;
    let forming: SessionId = "claude-code:00000000-0000-4000-8000-000000000052".parse()?;
    let placement = format!("/api/sessions/{}/placement", encode_segment(&filed));
    let _: PlacementReply = api
        .json(
            "PUT",
            &placement,
            &format!(r#"{{"topicId":"{}"}}"#, release.id),
            StatusCode::OK,
        )
        .await?;
    let history = format!("/api/sessions/{}/history", encode_segment(&filed));
    let _: SessionHistoryMark = api.json("PUT", &history, "", StatusCode::OK).await?;
    api.store
        .suggest(Suggestion {
            session: forming.clone(),
            name: TopicName::new("Mesh Certificates")?,
            topic: None,
            parent: None,
        })
        .await?;
    let all: TopicFiling = api
        .json("GET", "/api/topics/filing", "", StatusCode::OK)
        .await?;
    let one: TopicSessionList = api
        .json(
            "GET",
            &format!("/api/topics/{}/sessions", release.id),
            "",
            StatusCode::OK,
        )
        .await?;
    assert_eq!(all.sessions, one.sessions);
    assert_eq!(
        all.sessions
            .iter()
            .map(|row| (&row.placement.session_id, row.placement.by))
            .collect::<Vec<_>>(),
        [(&filed, PlacedBy::You)]
    );
    let marks: SessionHistoryList = api
        .json("GET", "/api/sessions/history", "", StatusCode::OK)
        .await?;
    assert_eq!(all.history, marks.sessions);
    let suggestions: SessionSuggestionList = api
        .json("GET", "/api/sessions/suggestions", "", StatusCode::OK)
        .await?;
    let shown: PlacementReply = api
        .json(
            "GET",
            &format!("/api/sessions/{}/placement", encode_segment(&forming)),
            "",
            StatusCode::OK,
        )
        .await?;
    assert_eq!(
        suggestions.sessions,
        [SessionSuggestion {
            session_id: forming,
            suggestion: shown.suggestion.ok_or("the session has a suggestion")?,
        }]
    );
    Ok(())
}
