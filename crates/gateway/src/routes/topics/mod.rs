mod map;

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::Path;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing;
use chrono::Utc;
use kernel::topics::Change;
use kernel::topics::NewTopic;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::Topic;
use kernel::topics::TopicChanges;
use kernel::topics::TopicId;
use kernel::topics::TopicKey;
use kernel::topics::TopicName;
use kernel::topics::TopicQuery;
use kernel::topics::TopicStore;
use serde::Deserialize;
use wire::gateway::topics::PlacementReply;
use wire::gateway::topics::PlacementRequest;
use wire::gateway::topics::TopicCreateRequest;
use wire::gateway::topics::TopicList;
use wire::gateway::topics::TopicMergeRequest;
use wire::gateway::topics::TopicRecord;
use wire::gateway::topics::TopicSessionList;
use wire::gateway::topics::TopicUpdate;
use wire::session_id::SessionId;
use wire::session_id::SessionRef;

use crate::Gateway;
use crate::error::ApiError;
use crate::extract::Body;
use crate::extract::Query;

const SEARCH_LIMIT_DEFAULT: usize = 10;
const SEARCH_LIMIT_MAX: usize = 50;

pub(crate) fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route(
            "/api/topics",
            routing::get(list)
                .post(create)
                .fallback(super::method_not_allowed),
        )
        .route(
            "/api/topics/search",
            routing::get(search).fallback(super::method_not_allowed),
        )
        .route(
            "/api/topics/{topic}",
            routing::get(show)
                .patch(update)
                .fallback(super::method_not_allowed),
        )
        .route(
            "/api/topics/{topic}/merge",
            routing::post(merge).fallback(super::method_not_allowed),
        )
        .route(
            "/api/topics/{topic}/sessions",
            routing::get(sessions).fallback(super::method_not_allowed),
        )
        .route(
            "/api/sessions/{session}/placement",
            routing::get(placement)
                .put(place)
                .delete(unplace)
                .fallback(super::method_not_allowed),
        )
        .layer(DefaultBodyLimit::max(super::BODY_LIMIT))
}

async fn resolve(store: &dyn TopicStore, key: &str) -> Result<Topic, ApiError> {
    let parsed = TopicKey::parse(key).map_err(map::error)?;
    store
        .find(&parsed)
        .await
        .map_err(map::error)?
        .ok_or_else(|| ApiError::Missing(format!("unknown topic {key}")))
}

async fn resolve_id(store: &dyn TopicStore, key: &str) -> Result<TopicId, ApiError> {
    Ok(resolve(store, key).await?.id)
}

fn canonical(segment: &str) -> Result<SessionId, ApiError> {
    let decoded = wire::session_id::decode_segment(segment)
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    match decoded.session {
        SessionRef::Canonical(session) => Ok(session),
        SessionRef::Bare(_) => Err(ApiError::BadRequest("canonical session required".into())),
    }
}

async fn list(State(gateway): State<Arc<Gateway>>) -> Result<Json<TopicList>, ApiError> {
    let listings = gateway.topics()?.list().await.map_err(map::error)?;
    Ok(Json(TopicList {
        topics: listings.into_iter().map(map::summary).collect(),
    }))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SearchParams {
    q: String,
    limit: Option<usize>,
}

async fn search(
    State(gateway): State<Arc<Gateway>>,
    Query(params): Query<SearchParams>,
) -> Result<Json<TopicList>, ApiError> {
    let limit = match params.limit {
        Some(limit) => limit.clamp(1, SEARCH_LIMIT_MAX),
        None => SEARCH_LIMIT_DEFAULT,
    };
    let hits = gateway
        .topics()?
        .search(&TopicQuery::new(&params.q), limit)
        .await
        .map_err(map::error)?;
    Ok(Json(TopicList {
        topics: hits.into_iter().map(map::summary).collect(),
    }))
}

async fn show(
    State(gateway): State<Arc<Gateway>>,
    Path(key): Path<String>,
) -> Result<Json<TopicRecord>, ApiError> {
    let topic = resolve(gateway.topics()?.as_ref(), &key).await?;
    Ok(Json(map::record(topic)))
}

async fn create(
    State(gateway): State<Arc<Gateway>>,
    Body(request): Body<TopicCreateRequest>,
) -> Result<(StatusCode, Json<TopicRecord>), ApiError> {
    let store = gateway.topics()?;
    let parent = match request.parent.as_deref() {
        Some(key) => Some(resolve_id(store.as_ref(), key).await?),
        None => None,
    };
    let new = NewTopic {
        name: TopicName::new(&request.name).map_err(map::error)?,
        aliases: map::aliases(&request.aliases)?,
        parent,
        directory: request.directory.map(map::root),
    };
    let topic = Topic::create(TopicId::random(), new, Utc::now()).map_err(map::error)?;
    let created = store.create(topic).await.map_err(map::error)?;
    Ok((StatusCode::CREATED, Json(map::record(created))))
}

async fn update(
    State(gateway): State<Arc<Gateway>>,
    Path(key): Path<String>,
    Body(request): Body<TopicUpdate>,
) -> Result<Json<TopicRecord>, ApiError> {
    let store = gateway.topics()?;
    let id = resolve_id(store.as_ref(), &key).await?;
    let parent = match request.parent {
        None => Change::Keep,
        Some(None) => Change::Clear,
        Some(Some(parent)) => Change::Set(resolve_id(store.as_ref(), &parent).await?),
    };
    let directory = match request.directory {
        None => Change::Keep,
        Some(None) => Change::Clear,
        Some(Some(directory)) => Change::Set(map::root(directory)),
    };
    let changes = TopicChanges {
        name: request
            .name
            .as_deref()
            .map(TopicName::new)
            .transpose()
            .map_err(map::error)?,
        aliases: request.aliases.as_deref().map(map::aliases).transpose()?,
        parent,
        directory,
    };
    let updated = store
        .update(id, changes, Utc::now())
        .await
        .map_err(map::error)?;
    Ok(Json(map::record(updated)))
}

async fn merge(
    State(gateway): State<Arc<Gateway>>,
    Path(key): Path<String>,
    Body(request): Body<TopicMergeRequest>,
) -> Result<Json<TopicRecord>, ApiError> {
    let store = gateway.topics()?;
    let source = resolve_id(store.as_ref(), &key).await?;
    let into = resolve_id(store.as_ref(), &request.into).await?;
    let merged = store
        .merge(source, into, Utc::now())
        .await
        .map_err(map::error)?;
    Ok(Json(map::record(merged)))
}

async fn sessions(
    State(gateway): State<Arc<Gateway>>,
    Path(key): Path<String>,
) -> Result<Json<TopicSessionList>, ApiError> {
    let store = gateway.topics()?;
    let id = resolve_id(store.as_ref(), &key).await?;
    let placed = store.sessions(id).await.map_err(map::error)?;
    Ok(Json(TopicSessionList {
        sessions: placed.into_iter().map(map::session).collect(),
    }))
}

async fn placement(
    State(gateway): State<Arc<Gateway>>,
    Path(segment): Path<String>,
) -> Result<Json<PlacementReply>, ApiError> {
    let session = canonical(&segment)?;
    let store = gateway.topics()?;
    let current = store.placement(&session).await.map_err(map::error)?;
    let suggestion = match current {
        Some(_) => None,
        None => store.suggestion(&session).await.map_err(map::error)?,
    };
    Ok(Json(PlacementReply {
        placement: current.map(map::placement),
        suggestion: suggestion.as_ref().map(map::suggestion),
    }))
}

async fn place(
    State(gateway): State<Arc<Gateway>>,
    Path(segment): Path<String>,
    Body(request): Body<PlacementRequest>,
) -> Result<Json<PlacementReply>, ApiError> {
    let session = canonical(&segment)?;
    let store = gateway.topics()?;
    let topic = resolve_id(store.as_ref(), &request.topic_id).await?;
    let outcome = store
        .place(Placement {
            session,
            topic,
            by: PlacementSource::You,
            at: Utc::now(),
        })
        .await
        .map_err(map::error)?;
    Ok(Json(PlacementReply {
        placement: Some(map::placement(outcome.into_placement())),
        suggestion: None,
    }))
}

async fn unplace(
    State(gateway): State<Arc<Gateway>>,
    Path(segment): Path<String>,
) -> Result<Json<PlacementReply>, ApiError> {
    let session = canonical(&segment)?;
    gateway
        .topics()?
        .unplace(&session)
        .await
        .map_err(map::error)?;
    Ok(Json(PlacementReply {
        placement: None,
        suggestion: None,
    }))
}
