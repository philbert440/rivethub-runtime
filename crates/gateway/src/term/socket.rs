use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use kernel::terminal::{TermSize, TerminalId};
use tokio::sync::oneshot;
use wire::events::ProtocolVersion;
use wire::gateway::term::{DeviceOwner, HelloState, PtyState, TermClientFrame, TermServerFrame};

use super::TerminalService;
use super::actor::{Command, Handle};
use crate::error::ApiError;
use crate::routes::sessions::{SocketError, send};

#[derive(Debug, thiserror::Error)]
pub(crate) enum TerminalSocketError {
    #[error(transparent)]
    Api(#[from] ApiError),
    #[error(transparent)]
    Socket(#[from] SocketError),
    #[error(transparent)]
    Transport(#[from] axum::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Terminal(#[from] kernel::terminal::TerminalMuxError),
}

pub(crate) async fn run(
    mut socket: WebSocket,
    service: Arc<TerminalService>,
    mut handle: Handle,
) -> Result<(), TerminalSocketError> {
    let client = uuid::Uuid::new_v4();
    let (reply, result) = oneshot::channel();
    handle.send(Command::Join { client, reply }).await?;
    result
        .await
        .map_err(|_| ApiError::Unavailable("terminal actor closed".into()))??;
    let result = stream(&mut socket, &service, &mut handle, client).await;
    handle.send(Command::Leave { client }).await?;
    if let Err(error) = &result {
        tracing::debug!(%error, "closing terminal websocket");
        socket
            .send(Message::Close(Some(CloseFrame {
                code: 1008,
                reason: "invalid terminal operation".into(),
            })))
            .await?;
    }
    result
}

async fn stream(
    socket: &mut WebSocket,
    service: &TerminalService,
    handle: &mut Handle,
    client: uuid::Uuid,
) -> Result<(), TerminalSocketError> {
    let state = handle
        .state
        .borrow()
        .clone()
        .ok_or(ApiError::NotFound("unknown pty"))?;
    let id = TerminalId::new(&state.id);
    let mut attached = service.mux.open(&id).await?;
    let owner = handle.ownership.borrow_and_update().clone();
    send(
        socket,
        &TermServerFrame::Hello {
            v: ProtocolVersion::V1,
            id: state.id,
            session: state.session,
            command: state.command,
            cols: state.cols.ok_or(ApiError::NotFound("terminal size"))?,
            rows: state.rows.ok_or(ApiError::NotFound("terminal size"))?,
            state: if state.state == PtyState::Running {
                HelloState::Running
            } else {
                HelloState::Exited
            },
            exit_code: state.exit_code,
            mux: state.mux,
            owner: owner.map(|owner| DeviceOwner {
                device: Some("another device".into()),
                self_owner: owner.client == client,
            }),
        },
    )
    .await?;
    socket.send(Message::Binary(attached.replay.into())).await?;
    loop {
        tokio::select! {
            () = service.shutdown.cancelled() => { socket.send(Message::Close(None)).await?; return Ok(()); }
            frame = socket.recv() => match frame {
                None | Some(Ok(Message::Close(_))) => return Ok(()),
                Some(Err(error)) => return Err(error.into()),
                Some(Ok(Message::Binary(bytes))) => service.mux.input(&id, bytes.to_vec()).await?,
                Some(Ok(Message::Text(text))) => control(service, handle, client, &id, &text).await?,
                Some(Ok(Message::Ping(data))) => socket.send(Message::Pong(data)).await?,
                Some(Ok(Message::Pong(_))) => {},
            },
            changed = handle.ownership.changed() => {
                changed.map_err(|_| ApiError::Unavailable("terminal owner channel closed".into()))?;
                let owner = handle.ownership.borrow_and_update().clone();
                send(socket, &TermServerFrame::Owner { device: Some("another device".into()), self_owner: owner.as_ref().is_some_and(|owner| owner.client == client), since: owner.map(|owner| owner.since) }).await?;
            }
            output = attached.output.recv() => {
                if let Some(bytes) = output {
                    socket.send(Message::Binary(bytes.into())).await?;
                } else {
                    let code = service.mux.wait(&id).await?;
                    send(socket, &TermServerFrame::Exit { code: Some(code), signal: None }).await?;
                    socket.send(Message::Close(Some(CloseFrame { code: 1000, reason: "".into() }))).await?;
                    return Ok(());
                }
            }
        }
    }
}

async fn control(
    service: &TerminalService,
    handle: &Handle,
    client: uuid::Uuid,
    id: &TerminalId,
    text: &str,
) -> Result<(), TerminalSocketError> {
    let frame: TermClientFrame = serde_json::from_str(text)?;
    let (size, claim) = match frame {
        TermClientFrame::Kill => {
            service.mux.kill(id).await?;
            return Ok(());
        }
        TermClientFrame::Resize { cols, rows } => (Some(TermSize { cols, rows }), false),
        TermClientFrame::Claim { cols, rows } => {
            let size = match (cols, rows) {
                (Some(cols), Some(rows)) => Some(TermSize { cols, rows }),
                (None, None) => None,
                (Some(_), None) | (None, Some(_)) => {
                    return Err(
                        ApiError::BadRequest("both cols and rows are required".into()).into(),
                    );
                }
            };
            (size, true)
        }
    };
    let (reply, result) = oneshot::channel();
    handle
        .send(Command::Size {
            client,
            size,
            claim,
            reply,
        })
        .await?;
    result
        .await
        .map_err(|_| ApiError::Unavailable("terminal actor closed".into()))??;
    Ok(())
}
