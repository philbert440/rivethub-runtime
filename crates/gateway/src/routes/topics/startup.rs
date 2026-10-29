use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::extract::State;
use chrono::Utc;
use kernel::BoxError;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::StartupContext;
use kernel::topics::Topic;
use kernel::topics::startup_context;
use wire::gateway::topics::StartupContext as StartupBody;
use wire::session_id::SessionId;

use super::map;
use crate::Gateway;
use crate::error::ApiError;
use crate::tenancy::UserId;
use crate::term::TerminalService;

pub(crate) async fn prepare(
    gateway: &Gateway,
    topic: &Topic,
    prompt: Option<&str>,
) -> Option<StartupContext> {
    match build(gateway, topic, prompt).await {
        Ok(context) => Some(context),
        Err(error) => {
            tracing::warn!(topic = %topic.id, "startup context was not built: {error}");
            None
        }
    }
}

async fn build(
    gateway: &Gateway,
    topic: &Topic,
    prompt: Option<&str>,
) -> Result<StartupContext, BoxError> {
    let (pages, _) = gateway
        .startup_memory()
        .ok_or("startup memory is not configured")?;
    let topics = gateway.topics()?;
    let memory = gateway.store();
    Ok(startup_context(
        topics.as_ref(),
        pages,
        memory.as_ref(),
        topic,
        prompt,
        Utc::now(),
    )
    .await?)
}

pub(crate) struct Started<'a> {
    pub(crate) session: &'a SessionId,
    pub(crate) owner: &'a str,
    pub(crate) topic: &'a Topic,
    pub(crate) context: Option<StartupContext>,
}

pub(crate) async fn begin(
    gateway: &Gateway,
    service: &Arc<TerminalService>,
    user: &UserId,
    started: Started<'_>,
) -> Result<Option<StartupBody>, ApiError> {
    let Started {
        session,
        owner,
        topic,
        context,
    } = started;
    let placed = gateway
        .topics()?
        .place(Placement {
            session: session.clone(),
            topic: topic.id,
            by: PlacementSource::You,
            at: Utc::now(),
        })
        .await;
    if let Err(error) = placed {
        if let Err(cleanup) = service.remove_session(user, owner).await {
            tracing::warn!(%session, "an unplaced new session was not removed: {cleanup}");
        }
        return Err(map::error(error));
    }
    let Some(context) = context else {
        return Ok(None);
    };
    let kept = match gateway.startup_memory() {
        Some((_, kept)) => kept.keep_startup(session, &context, Utc::now()).await,
        None => return Ok(None),
    };
    match kept {
        Ok(()) => Ok(Some((&context).into())),
        Err(error) => {
            tracing::warn!(%session, "startup context was not kept: {error}");
            Ok(None)
        }
    }
}

pub(super) async fn preview(
    State(gateway): State<Arc<Gateway>>,
    Path(key): Path<String>,
) -> Result<Json<StartupBody>, ApiError> {
    let topic = super::resolve(gateway.topics()?.as_ref(), &key).await?;
    let context = build(&gateway, &topic, None).await.map_err(|error| {
        ApiError::Unavailable(format!("startup context for {}: {error}", topic.slug))
    })?;
    Ok(Json((&context).into()))
}

pub(super) async fn kept(
    State(gateway): State<Arc<Gateway>>,
    Path(segment): Path<String>,
) -> Result<Json<StartupBody>, ApiError> {
    let session = super::canonical(&segment)?;
    let (_, kept) = gateway
        .startup_memory()
        .ok_or_else(|| ApiError::Unavailable("startup memory unavailable".into()))?;
    let context = kept
        .startup(&session)
        .await
        .map_err(map::error)?
        .ok_or_else(|| ApiError::Missing(format!("no startup context for {session}")))?;
    Ok(Json((&context).into()))
}
