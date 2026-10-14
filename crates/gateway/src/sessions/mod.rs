pub mod control;
mod view;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use wire::events::{
    AgentEvent, AgentEventKind, ProtocolVersion, SessionBroadcast, SessionRemoved, Snapshot,
    StateResponse,
};

use crate::error::ApiError;
use crate::tenancy::UserId;
use view::Projection;

#[derive(Debug)]
struct Tenant {
    views: Mutex<BTreeMap<String, Projection>>,
    events: broadcast::Sender<SessionBroadcast>,
}

impl Tenant {
    fn new() -> Self {
        Self {
            views: Mutex::new(BTreeMap::new()),
            events: broadcast::channel(256).0,
        }
    }

    fn publish(&self, event: SessionBroadcast) {
        if self.events.send(event).is_err() {
            tracing::trace!("session event retained in snapshot; no live subscribers");
        }
    }
}

#[derive(Debug)]
pub struct SessionRegistry {
    owners: RwLock<BTreeMap<String, UserId>>,
    tenants: RwLock<BTreeMap<UserId, Arc<Tenant>>>,
    pub(crate) tasks: TaskTracker,
    pub(crate) shutdown: CancellationToken,
    eviction: Duration,
}

impl SessionRegistry {
    #[must_use]
    pub fn with_shutdown(mut self, shutdown: CancellationToken) -> Self {
        self.shutdown = shutdown;
        self
    }
    pub fn new(eviction: Duration) -> Self {
        Self {
            owners: RwLock::new(BTreeMap::new()),
            tenants: RwLock::new(BTreeMap::new()),
            tasks: TaskTracker::new(),
            shutdown: CancellationToken::new(),
            eviction,
        }
    }

    pub fn claim(&self, user: &UserId, session: &str) -> Result<String, ApiError> {
        let key = session_key(session)?;
        let mut owners = self.owners.write();
        if let Some(owner) = owners.get(&key) {
            if owner != user {
                return Err(ApiError::SessionForbidden);
            }
        } else {
            owners.insert(key.clone(), user.clone());
        }
        drop(owners);
        Ok(key)
    }

    pub fn authorize(&self, user: &UserId, session: &str) -> Result<String, ApiError> {
        let key = session_key(session)?;
        match self.owners.read().get(&key) {
            Some(owner) if owner == user => Ok(key),
            Some(_) => Err(ApiError::SessionForbidden),
            None => Err(ApiError::NotFound("unknown session")),
        }
    }

    fn tenant(&self, user: &UserId) -> Arc<Tenant> {
        if let Some(tenant) = self.tenants.read().get(user) {
            return tenant.clone();
        }
        self.tenants
            .write()
            .entry(user.clone())
            .or_insert_with(|| Arc::new(Tenant::new()))
            .clone()
    }

    pub fn ingest(
        self: &Arc<Self>,
        user: &UserId,
        events: Vec<AgentEvent>,
    ) -> Result<(), ApiError> {
        if events.len() > 100 {
            return Err(ApiError::BadRequest("at most 100 events".into()));
        }
        let keys = events
            .iter()
            .map(|event| session_key(&event.session))
            .collect::<Result<Vec<_>, _>>()?;
        {
            let mut owners = self.owners.write();
            for key in &keys {
                if owners.get(key).is_some_and(|owner| owner != user) {
                    return Err(ApiError::SessionForbidden);
                }
            }
            for key in &keys {
                owners.insert(key.clone(), user.clone());
            }
        }
        let tenant = self.tenant(user);
        let mut views = tenant.views.lock();
        for (mut event, key) in events.into_iter().zip(keys) {
            event.session.clone_from(&key);
            let projection = views
                .entry(key.clone())
                .or_insert_with(|| Projection::new(key.clone()));
            projection.apply(&event.kind, Utc::now());
            let revision = projection.revision;
            let ended = matches!(event.kind, AgentEventKind::SessionEnd { .. });
            tenant.publish(SessionBroadcast::Event(event));
            if ended {
                let registry = self.clone();
                let user = user.clone();
                self.tasks.spawn(async move {
                    tokio::select! {
                        () = registry.shutdown.cancelled() => {},
                        () = tokio::time::sleep(registry.eviction) => registry.evict(&user, &key, revision),
                    }
                });
            }
        }
        drop(views);
        Ok(())
    }

    fn evict(&self, user: &UserId, key: &str, revision: u64) {
        let tenant = self.tenant(user);
        let mut views = tenant.views.lock();
        if views
            .get(key)
            .is_some_and(|view| view.revision == revision && view.view.ended)
        {
            views.remove(key);
            tenant.publish(SessionBroadcast::Removed(SessionRemoved {
                v: ProtocolVersion::V1,
                session: key.into(),
            }));
        }
        drop(views);
    }

    pub fn remove(&self, user: &UserId, session: &str) -> Result<(), ApiError> {
        let key = self.authorize(user, session)?;
        let tenant = self.tenant(user);
        let mut views = tenant.views.lock();
        views.remove(&key);
        tenant.publish(SessionBroadcast::Removed(SessionRemoved {
            v: ProtocolVersion::V1,
            session: key,
        }));
        drop(views);
        Ok(())
    }

    pub fn state(&self, user: &UserId, session: &str) -> Result<StateResponse, ApiError> {
        let key = self.authorize(user, session)?;
        let tenant = self.tenant(user);
        let views = tenant.views.lock();
        let state = views
            .get(&key)
            .ok_or(ApiError::NotFound("unknown session"))?
            .view
            .clone();
        drop(views);
        Ok(StateResponse {
            session: key,
            state,
        })
    }

    pub fn subscribe(
        &self,
        user: &UserId,
        filter: Option<&str>,
    ) -> Result<(Snapshot, broadcast::Receiver<SessionBroadcast>), ApiError> {
        let filter = filter.map(|key| self.authorize(user, key)).transpose()?;
        let tenant = self.tenant(user);
        let views = tenant.views.lock();
        let receiver = tenant.events.subscribe();
        let mut sessions: Vec<_> = views.values().map(|view| view.summary.clone()).collect();
        sessions.sort_by(|a, b| {
            b.last_event_ts
                .cmp(&a.last_event_ts)
                .then_with(|| a.id.cmp(&b.id))
        });
        let projected = views
            .iter()
            .filter(|(key, _)| filter.as_ref().is_none_or(|filter| filter == *key))
            .map(|(key, value)| (key.clone(), value.view.clone()))
            .collect();
        drop(views);
        Ok((
            Snapshot {
                v: ProtocolVersion::V1,
                sessions,
                views: projected,
            },
            receiver,
        ))
    }

    pub fn bind_terminal(
        &self,
        user: &UserId,
        session: &str,
        terminal: &str,
    ) -> Result<(), ApiError> {
        let key = self.authorize(user, session)?;
        let tenant = self.tenant(user);
        let mut views = tenant.views.lock();
        if let Some(view) = views.get_mut(&key) {
            view.summary.pty = Some(terminal.into());
        }
        drop(views);
        Ok(())
    }

    pub fn count(&self) -> usize {
        self.tenants
            .read()
            .values()
            .map(|tenant| tenant.views.lock().len())
            .sum()
    }

    pub async fn close(&self) {
        self.shutdown.cancel();
        self.tasks.close();
        self.tasks.wait().await;
    }
}

pub fn session_key(value: &str) -> Result<String, ApiError> {
    let value = if let Some((prefix, native)) = value.split_once(':')
        && prefix.parse::<wire::harness::HarnessId>().is_ok()
    {
        native
    } else {
        value
    };
    if value.is_empty()
        || value.chars().count() > 160
        || value
            .chars()
            .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')))
        || value.contains("..")
    {
        return Err(ApiError::BadRequest(format!("invalid session id: {value}")));
    }
    Ok(value.into())
}
