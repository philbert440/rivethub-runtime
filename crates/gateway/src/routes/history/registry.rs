use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{FromRequestParts, RawQuery, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use tokio::sync::broadcast;
use tokio::time::{Instant, MissedTickBehavior};
use wire::harness::{HarnessEvent, HarnessId};
use wire::session_id::SessionId;

use super::first_param;
use crate::Gateway;
use crate::error::ApiError;
use crate::routes::sessions::{SocketError, send};
use crate::tenancy::UserId;
use crate::term::live::RegistryEvent;

const HEARTBEAT: Duration = Duration::from_secs(30);
const SEND_BUFFER: usize = 1024 * 1024;

pub(super) async fn upgrade(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    RawQuery(query): RawQuery,
    request: axum::extract::Request,
) -> Response {
    let harness = match harness_filter(query.as_deref()) {
        Ok(harness) => harness,
        Err(error) => return error.into_response(),
    };
    let (mut parts, body) = request.into_parts();
    drop(body);
    let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    let events = gateway
        .terminal()
        .map(|terminal| terminal.subscribe_registry());
    upgrade
        .max_write_buffer_size(SEND_BUFFER)
        .on_upgrade(move |socket| async move {
            let watcher = Watcher {
                socket,
                gateway: &gateway,
                user: &user,
                harness,
            };
            if let Err(error) = watcher.stream(events).await {
                tracing::debug!(%error, "harness registry socket ended");
            }
        })
        .into_response()
}

fn harness_filter(query: Option<&str>) -> Result<Option<HarnessId>, ApiError> {
    match first_param(query, "harness")? {
        None => Ok(None),
        Some(raw) => raw
            .parse::<HarnessId>()
            .map(Some)
            .map_err(|_| ApiError::BadRequest(format!("unknown harness: {raw}"))),
    }
}

fn session_of(event: &HarnessEvent) -> Option<&SessionId> {
    match event {
        HarnessEvent::SessionCreated { session_id, .. }
        | HarnessEvent::SessionUpdated { session_id, .. } => Some(session_id),
        HarnessEvent::AssistantDelta { .. }
        | HarnessEvent::ReasoningDelta { .. }
        | HarnessEvent::ToolUse { .. }
        | HarnessEvent::ToolUpdate { .. }
        | HarnessEvent::ToolResult { .. }
        | HarnessEvent::ApprovalRequest { .. }
        | HarnessEvent::ApprovalResolved { .. }
        | HarnessEvent::Plan { .. }
        | HarnessEvent::TurnComplete { .. }
        | HarnessEvent::Error { .. }
        | HarnessEvent::Context { .. }
        | HarnessEvent::Transcript { .. }
        | HarnessEvent::Status(_)
        | HarnessEvent::Driver { .. }
        | HarnessEvent::Prompt { .. } => None,
    }
}

fn served_by(session: &SessionId, harness: HarnessId) -> bool {
    session.harness() == harness
}

async fn next_event(
    events: &mut Option<broadcast::Receiver<RegistryEvent>>,
) -> Result<RegistryEvent, broadcast::error::RecvError> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

struct Watcher<'a> {
    socket: WebSocket,
    gateway: &'a Gateway,
    user: &'a UserId,
    harness: Option<HarnessId>,
}

impl Watcher<'_> {
    fn wants(&self, registry: &RegistryEvent) -> bool {
        let Some(session) = session_of(&registry.event) else {
            return false;
        };
        self.harness
            .is_none_or(|harness| served_by(session, harness))
            && self
                .gateway
                .sessions()
                .authorize(self.user, &registry.key)
                .is_ok()
    }

    async fn stream(
        mut self,
        mut events: Option<broadcast::Receiver<RegistryEvent>>,
    ) -> Result<(), SocketError> {
        let mut heartbeat = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut awaiting_pong = false;
        loop {
            tokio::select! {
                () = self.gateway.sessions().shutdown.cancelled() => {
                    self.socket.send(Message::Close(None)).await?;
                    return Ok(());
                }
                frame = self.socket.recv() => match frame {
                    Some(Ok(Message::Close(_))) | None => return Ok(()),
                    Some(Err(error)) => return Err(error.into()),
                    Some(Ok(Message::Ping(data))) => self.socket.send(Message::Pong(data)).await?,
                    Some(Ok(Message::Pong(_))) => awaiting_pong = false,
                    Some(Ok(Message::Text(_) | Message::Binary(_))) => {}
                },
                _ = heartbeat.tick() => {
                    if awaiting_pong {
                        return Ok(());
                    }
                    awaiting_pong = true;
                    self.socket.send(Message::Ping(Vec::new().into())).await?;
                }
                event = next_event(&mut events) => match event {
                    Ok(registry) => {
                        if self.wants(&registry) {
                            send(&mut self.socket, &registry.event).await?;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => events = None,
                    Err(broadcast::error::RecvError::Lagged(_)) => return Err(SocketError::Lagged),
                },
            }
        }
    }
}
