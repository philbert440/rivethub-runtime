use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use kernel::memory::{
    EMPTY_SESSIONS_MAX, SessionActivity, SessionDeletion, Speaker, SpokenTurn, TrashReason,
    TrashSignals, trash_verdict,
};
use kernel::transcript::{ListLimit, StoreSession};
use wire::gateway::session_delete::{SessionTrashCandidate, SessionTrashList, SessionTrashReason};
use wire::gateway::sessions::{HarnessTranscriptTurn, SessionMessageRole};
use wire::harness::HarnessId;
use wire::session_id::SessionId;

use super::{deletions, live, resolved, visible};
use crate::Gateway;
use crate::error::ApiError;
use crate::routes::terminal::require;
use crate::tenancy::UserId;

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

async fn listed(gateway: &Gateway, user: &UserId, roster: &[HarnessId]) -> Vec<StoreSession> {
    let rows = match gateway.session_lister() {
        Some(lister) => lister.list(roster, ListLimit::most()).await,
        None => Vec::new(),
    };
    crate::routes::history::visible_rows(gateway, user, rows)
}

async fn unlisted_empty(
    gateway: &Gateway,
    user: &UserId,
    deletions: &dyn SessionDeletion,
    roster: &[HarnessId],
    shown: &BTreeSet<SessionId>,
) -> Result<Vec<StoreSession>, ApiError> {
    let Some(lister) = gateway.session_lister() else {
        return Ok(Vec::new());
    };
    let mut rows = Vec::new();
    for key in deletions.empty_sessions(EMPTY_SESSIONS_MAX).await? {
        #[cfg_attr(
            dylint_lib = "rr_lints",
            expect(
                fallback,
                reason = "CONTRACT B1: a memory session key that is not a canonical session id is never a trash candidate (note 75)"
            )
        )]
        let Ok(session) = key.parse::<SessionId>() else {
            continue;
        };
        if shown.contains(&session)
            || !roster.contains(&session.harness())
            || !visible(gateway, user, &session).await?
        {
            continue;
        }
        if let Some(row) = lister
            .describe(session.harness(), session.native(), None)
            .await?
        {
            rows.push(row);
        }
    }
    Ok(rows)
}

pub(super) async fn trash(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
) -> Result<Json<SessionTrashList>, ApiError> {
    let deletions = deletions(&gateway)?;
    let roster = require(&gateway)?.roster();
    let now = Utc::now();
    let mut rows = listed(&gateway, &user, &roster).await;
    let seen: BTreeSet<SessionId> = rows
        .iter()
        .map(|row| SessionId::new(row.harness, &row.id))
        .collect();
    rows.extend(unlisted_empty(&gateway, &user, deletions, &roster, &seen).await?);
    let rows: Vec<Considered> = rows
        .into_iter()
        .filter_map(|row| considered(row, now))
        .filter(|row| !live(&gateway, &row.session))
        .collect();
    let keys: Vec<String> = rows
        .iter()
        .map(|row| row.session.as_str().to_owned())
        .collect();
    let deleted = deletions.tombstones(&keys).await?;
    let activity = deletions.session_activity(&keys).await?;
    let mut candidates = Vec::new();
    for row in rows {
        let key = row.session.as_str();
        if deleted.contains_key(key) {
            continue;
        }
        let Some(memory) = activity.get(key) else {
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

fn spoken(turn: &HarnessTranscriptTurn) -> SpokenTurn<'_> {
    SpokenTurn {
        speaker: match turn.role {
            SessionMessageRole::User => Speaker::User,
            SessionMessageRole::Assistant => Speaker::Assistant,
        },
        text: &turn.text,
        worked: turn.tools.as_ref().is_some_and(|tools| !tools.is_empty())
            || turn
                .attachments
                .as_ref()
                .is_some_and(|attachments| !attachments.is_empty()),
    }
}

const fn reason(reason: TrashReason) -> SessionTrashReason {
    match reason {
        TrashReason::Empty => SessionTrashReason::Empty,
        TrashReason::OneWordReply => SessionTrashReason::OneWordReply,
    }
}

async fn judged(
    gateway: &Gateway,
    row: &Considered,
    memory: &SessionActivity,
    now: DateTime<Utc>,
) -> Option<SessionTrashCandidate> {
    let read = gateway
        .history()
        .transcript(gateway, &resolved(&row.session))
        .await;
    let turns = match read {
        Ok(transcript) => transcript.read.turns,
        Err(error) => {
            tracing::debug!(session = %row.session, %error, "transcript unreadable; not a trash candidate");
            return None;
        }
    };
    let transcript: Vec<SpokenTurn<'_>> = turns.iter().map(spoken).collect();
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
        reason: reason(verdict.reason),
        user_turns: verdict.user_turns,
        reply: verdict.reply,
    })
}
