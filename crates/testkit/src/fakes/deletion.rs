use std::collections::BTreeMap;
use std::collections::BTreeSet;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::memory::DeletedSession;
use kernel::memory::MemoryStoreError;
use kernel::memory::SessionActivity;
use kernel::memory::SessionDeletion;
use parking_lot::Mutex;
use wire::session_id::SessionId;

#[derive(Debug, Default)]
pub struct FakeSessionDeletion {
    activity: Mutex<BTreeMap<SessionId, SessionActivity>>,
    tombstones: Mutex<BTreeMap<SessionId, Option<String>>>,
}

impl FakeSessionDeletion {
    #[must_use]
    pub fn with_activity(self, session: SessionId, activity: SessionActivity) -> Self {
        self.activity.lock().insert(session, activity);
        self
    }

    pub fn tombstone(&self, session: SessionId) {
        self.tombstones.lock().insert(session, None);
    }

    #[must_use]
    pub fn activity_of(&self, session: &SessionId) -> Option<SessionActivity> {
        self.activity.lock().get(session).cloned()
    }

    #[must_use]
    pub fn deleted_by(&self, session: &SessionId) -> Option<Option<String>> {
        self.tombstones.lock().get(session).cloned()
    }
}

#[async_trait]
impl SessionDeletion for FakeSessionDeletion {
    async fn delete_session(
        &self,
        session: &SessionId,
        deleted_by: Option<&str>,
        _now: DateTime<Utc>,
    ) -> Result<DeletedSession, MemoryStoreError> {
        self.tombstones
            .lock()
            .insert(session.clone(), deleted_by.map(str::to_owned));
        let removed = self.activity.lock().remove(session);
        Ok(match removed {
            Some(removed) => DeletedSession {
                conversations: removed.conversations,
                messages: removed.messages,
                summaries: 0,
            },
            None => DeletedSession::default(),
        })
    }

    async fn tombstoned(
        &self,
        sessions: &[SessionId],
    ) -> Result<BTreeSet<SessionId>, MemoryStoreError> {
        let tombstones = self.tombstones.lock();
        Ok(sessions
            .iter()
            .filter(|session| tombstones.contains_key(*session))
            .cloned()
            .collect())
    }

    async fn session_activity(
        &self,
        sessions: &[SessionId],
    ) -> Result<BTreeMap<SessionId, SessionActivity>, MemoryStoreError> {
        let activity = self.activity.lock();
        Ok(sessions
            .iter()
            .map(|session| match activity.get(session) {
                Some(found) => (session.clone(), found.clone()),
                None => (session.clone(), SessionActivity::default()),
            })
            .collect())
    }

    async fn purge_tombstoned(&self, _limit: u32) -> Result<DeletedSession, MemoryStoreError> {
        Ok(DeletedSession::default())
    }
}
