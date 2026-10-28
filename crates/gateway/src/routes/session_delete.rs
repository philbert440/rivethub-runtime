use std::collections::BTreeSet;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use kernel::memory::{
    DeletedSession, SessionActivity, SessionDeletion, TrashSignals, trash_verdict,
};
use kernel::transcript::{ListLimit, StoreSession};
use wire::gateway::session_delete::{
    SessionDeleteError, SessionDeleteRefused, SessionDeleteRequest, SessionDeleteResult,
    SessionDeleteResults, SessionDeleted, SessionTrashCandidate, SessionTrashList,
};
use wire::session_id::{SessionId, SessionRef, decode_segment};

use super::terminal::require;
use crate::Gateway;
use crate::error::ApiError;
use crate::extract::Body;
use crate::history::{Resolved, authorize};
use crate::tenancy::UserId;

const MAX_BATCH: usize = 200;
const LIVE: &str = "session is live";
const NOT_FOUND: &str = "not found";

pub(crate) fn router() -> Router<Arc<Gateway>> {
    Router::new()
        .route(
            "/api/sessions/delete",
            post(delete_many).fallback(super::method_not_allowed),
        )
        .route(
            "/api/sessions/trash",
            get(trash).fallback(super::method_not_allowed),
        )
        .route(
            "/api/sessions/{session}",
            delete(delete_one).fallback(super::method_not_allowed),
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    Live,
    NotFound,
}

fn deletions(gateway: &Gateway) -> Result<&dyn SessionDeletion, ApiError> {
    gateway
        .session_deletion()
        .ok_or_else(|| ApiError::Unavailable("session deletion unavailable".to_owned()))
}

fn segment_session(segment: &str) -> Result<SessionId, ApiError> {
    if let Ok(session) = segment.parse::<SessionId>() {
        return Ok(session);
    }
    let decoded =
        decode_segment(segment).map_err(|error| ApiError::BadRequest(error.to_string()))?;
    match decoded.session {
        SessionRef::Canonical(session) => Ok(session),
        SessionRef::Bare(_) => Err(ApiError::BadRequest(
            "canonical session required".to_owned(),
        )),
    }
}

fn resolved(session: &SessionId) -> Resolved {
    Resolved {
        session: session.clone(),
        harness: session.harness(),
        native: session.native().to_owned(),
        redirected_to: None,
    }
}

fn live(gateway: &Gateway, session: &SessionId) -> bool {
    gateway
        .terminal()
        .is_some_and(|terminal| terminal.is_live(session.native()))
}

async fn visible(gateway: &Gateway, user: &UserId, session: &SessionId) -> Result<bool, ApiError> {
    match authorize(gateway, user, &resolved(session)).await {
        Ok(()) => Ok(true),
        Err(ApiError::SessionForbidden) => Ok(false),
        Err(error) => Err(error),
    }
}

async fn known(
    gateway: &Gateway,
    deletions: &dyn SessionDeletion,
    session: &SessionId,
) -> Result<bool, ApiError> {
    if let Some(lister) = gateway.session_lister()
        && lister
            .describe(session.harness(), session.native(), None)
            .await?
            .is_some()
    {
        return Ok(true);
    }
    let activity = deletions
        .session_activity(std::slice::from_ref(session))
        .await?;
    Ok(activity.get(session).is_some_and(SessionActivity::recorded))
}

async fn erase(
    gateway: &Gateway,
    user: &UserId,
    session: &SessionId,
) -> Result<Result<DeletedSession, Refusal>, ApiError> {
    let deletions = deletions(gateway)?;
    if !visible(gateway, user, session).await? {
        return Ok(Err(Refusal::NotFound));
    }
    if live(gateway, session) {
        return Ok(Err(Refusal::Live));
    }
    if !known(gateway, deletions, session).await? {
        return Ok(Err(Refusal::NotFound));
    }
    let deleted = deletions
        .delete_session(session, Some(user.as_str()), Utc::now())
        .await?;
    tracing::info!(
        session = %session,
        conversations = deleted.conversations,
        messages = deleted.messages,
        summaries = deleted.summaries,
        "session deleted"
    );
    Ok(Ok(deleted))
}

fn deleted_reply(session: SessionId, deleted: DeletedSession) -> SessionDeleted {
    SessionDeleted {
        ok: true,
        session,
        conversations: deleted.conversations,
        messages: deleted.messages,
        summaries: deleted.summaries,
    }
}

async fn delete_one(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Path(segment): Path<String>,
) -> Result<Json<SessionDeleted>, ApiError> {
    let session = segment_session(&segment)?;
    match erase(&gateway, &user, &session).await? {
        Ok(deleted) => Ok(Json(deleted_reply(session, deleted))),
        Err(Refusal::Live) => Err(ApiError::Conflict(LIVE.to_owned())),
        Err(Refusal::NotFound) => Err(ApiError::NotFound(NOT_FOUND)),
    }
}

fn refused(session: SessionId, error: SessionDeleteError) -> SessionDeleteResult {
    SessionDeleteResult::Refused(SessionDeleteRefused {
        session,
        ok: false,
        error,
    })
}

async fn delete_many(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    Body(request): Body<SessionDeleteRequest>,
) -> Result<Json<SessionDeleteResults>, ApiError> {
    deletions(&gateway)?;
    let mut seen = BTreeSet::new();
    let sessions: Vec<SessionId> = request
        .sessions
        .into_iter()
        .filter(|session| seen.insert(session.clone()))
        .collect();
    if sessions.is_empty() || sessions.len() > MAX_BATCH {
        return Err(ApiError::BadRequest(format!(
            "sessions must name 1 to {MAX_BATCH} sessions"
        )));
    }
    let mut results = Vec::with_capacity(sessions.len());
    for session in sessions {
        let result = match erase(&gateway, &user, &session).await {
            Ok(Ok(deleted)) => SessionDeleteResult::Deleted(deleted_reply(session, deleted)),
            Ok(Err(Refusal::Live)) => refused(session, SessionDeleteError::Live),
            Ok(Err(Refusal::NotFound)) | Err(ApiError::BadRequest(_)) => {
                refused(session, SessionDeleteError::NotFound)
            }
            Err(error) => {
                tracing::warn!(session = %session, %error, "session delete failed");
                refused(session, SessionDeleteError::Failed)
            }
        };
        results.push(result);
    }
    Ok(Json(SessionDeleteResults { results }))
}

struct Considered {
    row: StoreSession,
    session: SessionId,
    updated_at: DateTime<Utc>,
}

fn considered(row: StoreSession, now: DateTime<Utc>) -> Option<Considered> {
    let updated_at = DateTime::from_timestamp_millis(row.updated_at)?;
    let settled = row.updated_at > 0 && row.parent_session_id.is_none() && now >= updated_at;
    settled.then(|| Considered {
        session: SessionId::new(row.harness, &row.id),
        updated_at,
        row,
    })
}

async fn trash(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
) -> Result<Json<SessionTrashList>, ApiError> {
    let deletions = deletions(&gateway)?;
    let terminal = require(&gateway)?;
    let now = Utc::now();
    let rows = match gateway.session_lister() {
        Some(lister) => lister.list(&terminal.roster(), ListLimit::most()).await,
        None => Vec::new(),
    };
    let rows = super::history::visible_rows(&gateway, &user, rows);
    let rows: Vec<Considered> = rows
        .into_iter()
        .filter_map(|row| considered(row, now))
        .filter(|row| !live(&gateway, &row.session))
        .collect();
    let ids: Vec<SessionId> = rows.iter().map(|row| row.session.clone()).collect();
    let deleted = deletions.tombstones(&ids).await?;
    let activity = deletions.session_activity(&ids).await?;
    let mut candidates = Vec::new();
    for row in rows {
        if deleted.contains_key(&row.session) {
            continue;
        }
        let Some(memory) = activity.get(&row.session) else {
            continue;
        };
        if memory.kept() || memory.user_messages > 1 || memory.assistant_messages > 1 {
            continue;
        }
        if let Some(candidate) = judged(&gateway, &row, memory, now).await {
            candidates.push(candidate);
        }
    }
    Ok(Json(SessionTrashList { candidates }))
}

async fn judged(
    gateway: &Gateway,
    row: &Considered,
    memory: &SessionActivity,
    now: DateTime<Utc>,
) -> Option<SessionTrashCandidate> {
    let transcript = match gateway
        .history()
        .transcript(gateway, &resolved(&row.session))
        .await
    {
        Ok(transcript) => transcript.read.turns,
        Err(error) => {
            tracing::debug!(session = %row.session, %error, "transcript unreadable; not a trash candidate");
            return None;
        }
    };
    let title = (row.row.title != row.row.id).then_some(row.row.title.as_str());
    let verdict = trash_verdict(&TrashSignals {
        native: &row.row.id,
        title,
        updated_at: row.updated_at,
        now,
        activity: memory,
        transcript: &transcript,
    })?;
    Some(SessionTrashCandidate {
        session: row.session.clone(),
        harness: row.row.harness,
        title: title.map(str::to_owned),
        updated_at: row.updated_at,
        reason: verdict.reason,
        user_turns: verdict.user_turns,
        reply: verdict.reply,
    })
}
