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
use kernel::topics::TopicStore;
use kernel::topics::startup_context;
use wire::gateway::topics::StartupContext as StartupBody;
use wire::harness::HarnessSessionSummary;
use wire::session_id::SessionId;

use super::map;
use crate::Gateway;
use crate::error::ApiError;
use crate::tenancy::UserId;
use crate::term::TerminalService;

pub(crate) async fn begin(
    gateway: &Gateway,
    service: &Arc<TerminalService>,
    user: &UserId,
    summary: &mut HarnessSessionSummary,
    topic: &Topic,
    prompt: Option<&str>,
) -> Result<(), ApiError> {
    let topics = gateway.topics()?;
    let session = summary.session_id.clone();
    let placed = topics
        .place(Placement {
            session: session.clone(),
            topic: topic.id,
            by: PlacementSource::You,
            at: Utc::now(),
        })
        .await;
    if let Err(error) = placed {
        if let Err(cleanup) = service.remove_session(user, session.as_str()).await {
            tracing::warn!(%session, "an unplaced new session was not removed: {cleanup}");
        }
        return Err(map::error(error));
    }
    summary.startup_context = match build(gateway, topics.as_ref(), &session, topic, prompt).await {
        Ok(context) => Some((&context).into()),
        Err(error) => {
            tracing::warn!(%session, "startup context was not built: {error}");
            None
        }
    };
    Ok(())
}

async fn build(
    gateway: &Gateway,
    topics: &dyn TopicStore,
    session: &SessionId,
    topic: &Topic,
    prompt: Option<&str>,
) -> Result<StartupContext, BoxError> {
    let (pages, kept) = gateway
        .startup_memory()
        .ok_or("startup memory is not configured")?;
    let now = Utc::now();
    let context =
        startup_context(topics, pages, gateway.store().as_ref(), topic, prompt, now).await?;
    kept.keep_startup(session, &context, now).await?;
    Ok(context)
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
