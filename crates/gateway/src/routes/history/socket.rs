use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{FromRequestParts, RawQuery, State, WebSocketUpgrade};
use axum::response::{IntoResponse, Response};
use tokio::sync::broadcast;
use tokio::time::{Instant, MissedTickBehavior};
use wire::gateway::sessions::{
    HarnessSocketMessage, HarnessSocketRefusal, HarnessSocketRefusalCode,
};
use wire::harness::{HarnessCode, HarnessEvent, RosterCommand};

use super::first_param;
use crate::Gateway;
use crate::error::ApiError;
use crate::history::{Resolved, authorize};
use crate::routes::sessions::{SocketError, send};
use crate::tenancy::UserId;

const HEARTBEAT: Duration = Duration::from_secs(30);
const SYNC_INTERVAL: Duration = Duration::from_secs(2);
const SEND_BUFFER: usize = 1024 * 1024;

pub(super) async fn upgrade(
    State(gateway): State<Arc<Gateway>>,
    user: UserId,
    RawQuery(query): RawQuery,
    request: axum::extract::Request,
) -> Response {
    let segment = match first_param(query.as_deref(), "session") {
        Ok(Some(segment)) if !segment.is_empty() => segment,
        Ok(Some(_) | None) => {
            return ApiError::BadRequest("session is required".to_owned()).into_response();
        }
        Err(error) => return error.into_response(),
    };
    let (mut parts, body) = request.into_parts();
    drop(body);
    let upgrade = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(upgrade) => upgrade,
        Err(rejection) => return rejection.into_response(),
    };
    upgrade
        .max_write_buffer_size(SEND_BUFFER)
        .on_upgrade(move |socket| async move {
            if let Err(error) = attach(socket, gateway, user, segment).await {
                tracing::debug!(%error, "harness session socket ended");
            }
        })
        .into_response()
}

async fn attach(
    mut socket: WebSocket,
    gateway: Arc<Gateway>,
    user: UserId,
    segment: String,
) -> Result<(), SocketError> {
    let resolved = match open(&gateway, &user, &segment).await {
        Ok(resolved) => resolved,
        Err(refusal) => {
            send(&mut socket, &refusal).await?;
            socket.send(Message::Close(None)).await?;
            return Ok(());
        }
    };
    let events = gateway
        .terminal()
        .map(|terminal| terminal.subscribe_harness(&resolved.native));
    let result = Attached {
        socket,
        gateway: &gateway,
        resolved: &resolved,
        rev: 0,
    }
    .stream(events)
    .await;
    if let Some(terminal) = gateway.terminal() {
        terminal.release_harness(&resolved.native);
    }
    result
}

async fn open(
    gateway: &Gateway,
    user: &UserId,
    segment: &str,
) -> Result<Resolved, HarnessSocketRefusal> {
    let resolved = gateway
        .history()
        .resolve(gateway, segment)
        .await
        .map_err(|error| refusal(HarnessSocketRefusalCode::InvalidSessionId, &error))?;
    authorize(gateway, user, &resolved).await.map_err(|error| {
        let code = if matches!(error, ApiError::SessionForbidden) {
            HarnessSocketRefusalCode::Forbidden
        } else {
            HarnessSocketRefusalCode::InvalidSessionId
        };
        refusal(code, &error)
    })?;
    Ok(resolved)
}

fn refusal(code: HarnessSocketRefusalCode, error: &ApiError) -> HarnessSocketRefusal {
    HarnessSocketRefusal::Error {
        code,
        message: error.to_string(),
    }
}

struct Attached<'a> {
    socket: WebSocket,
    gateway: &'a Gateway,
    resolved: &'a Resolved,
    rev: u64,
}

async fn next_event(
    events: &mut Option<broadcast::Receiver<HarnessEvent>>,
) -> Result<HarnessEvent, broadcast::error::RecvError> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

impl Attached<'_> {
    async fn stream(
        mut self,
        mut events: Option<broadcast::Receiver<HarnessEvent>>,
    ) -> Result<(), SocketError> {
        let mut heartbeat = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
        heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut awaiting_pong = false;
        let mut last_sync: Option<Instant> = None;
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
                    Some(Ok(Message::Text(text))) => {
                        if serde_json::from_str::<HarnessSocketMessage>(&text).is_ok_and(|message| message == HarnessSocketMessage::Sync)
                            && last_sync.is_none_or(|at| at.elapsed() >= SYNC_INTERVAL)
                        {
                            last_sync = Some(Instant::now());
                            self.snapshot().await?;
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {}
                },
                _ = heartbeat.tick() => {
                    if awaiting_pong {
                        return Ok(());
                    }
                    awaiting_pong = true;
                    self.socket.send(Message::Ping(Vec::new().into())).await?;
                }
                event = next_event(&mut events) => match event {
                    Ok(event) => {
                        let committed = matches!(event, HarnessEvent::TurnComplete { .. });
                        send(&mut self.socket, &event).await?;
                        if committed {
                            self.snapshot().await?;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => events = None,
                    Err(broadcast::error::RecvError::Lagged(_)) => return Err(SocketError::Lagged),
                },
            }
        }
    }

    async fn snapshot(&mut self) -> Result<(), SocketError> {
        let resolved = self.resolved;
        let read = match self
            .gateway
            .history()
            .transcript(self.gateway, resolved)
            .await
        {
            Ok(read) => read,
            Err(error) => {
                let code = if let ApiError::Code(code, _) = &error {
                    *code
                } else {
                    HarnessCode::BadRequest
                };
                return send(
                    &mut self.socket,
                    &HarnessEvent::Error {
                        session_id: resolved.session.clone(),
                        code: code.to_string(),
                        delivery_id: None,
                        message: error.to_string(),
                        retryable: None,
                    },
                )
                .await;
            }
        };
        self.rev += 1;
        let turns = read.read.turns;
        send(
            &mut self.socket,
            &HarnessEvent::Transcript {
                session_id: resolved.session.clone(),
                rev: self.rev,
                from: 0,
                total: turns.len() as u64,
                turns,
                command: RosterCommand::from(resolved.harness),
                truncated_before: read.read.truncated.then_some(true),
                context_window: Some(read.stamp.window),
                compact_at: Some(read.stamp.compact_at),
                context_source: Some(read.stamp.source),
            },
        )
        .await
    }
}
