use std::sync::Arc;

use axum::Extension;
use axum::extract::State;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::response::IntoResponse;
use axum::response::Response;
use tokio_util::sync::CancellationToken;
use wire::gateway::devices::AudioFormat;
use wire::gateway::devices::AudioMicClientFrame;
use wire::gateway::devices::AudioMicErrorCode;
use wire::gateway::devices::AudioMicServerFrame;

use super::bridge::ArmGuard;
use super::bridge::AudioCore;
use super::bridge::AudioFault;
use super::bridge::CHANNELS;
use crate::Gateway;
use crate::server::ConnectionAbort;

const FRAME_LIMIT: usize = 1024 * 1024;

pub(crate) async fn socket(
    State(gateway): State<Arc<Gateway>>,
    abort: Option<Extension<ConnectionAbort>>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
) -> Response {
    let Some(bridge) = gateway.audio() else {
        return if upgrade.is_ok() {
            abort_or_disabled(abort)
        } else {
            super::disabled_response()
        };
    };
    if let Ok(upgrade) = upgrade {
        accept(upgrade, bridge)
    } else {
        super::not_found_response()
    }
}

fn abort_or_disabled(abort: Option<Extension<ConnectionAbort>>) -> Response {
    match abort {
        Some(Extension(abort)) => {
            abort.trip();
            axum::http::StatusCode::NO_CONTENT.into_response()
        }
        None => super::disabled_response(),
    }
}

fn accept(upgrade: WebSocketUpgrade, bridge: &super::bridge::AudioBridge) -> Response {
    let core = bridge.core();
    let tasks = bridge.tasks();
    let shutdown = core.shutdown();
    upgrade
        .max_frame_size(FRAME_LIMIT)
        .max_message_size(FRAME_LIMIT)
        .on_upgrade(move |socket| async move {
            let join = tasks.spawn(drive(socket, core, shutdown));
            if let Err(error) = join.await {
                tracing::debug!("mic session task aborted: {error}");
            }
        })
        .into_response()
}

async fn drive(mut socket: WebSocket, core: AudioCore, shutdown: CancellationToken) {
    let mut guard = core.guard();
    let hello = match next_frame(&mut socket, &shutdown).await {
        Some(Message::Text(text)) => text,
        Some(Message::Binary(_)) => {
            fail(&mut socket, bad_hello("expected hello")).await;
            return;
        }
        Some(Message::Ping(_) | Message::Pong(_)) => {
            if !await_hello(&mut socket, &core, &shutdown).await {
                return;
            }
            return run(&mut socket, &core, &shutdown, &mut guard).await;
        }
        Some(Message::Close(_)) | None => return,
    };
    if !finish_hello(&mut socket, &core, &hello).await {
        return;
    }
    run(&mut socket, &core, &shutdown, &mut guard).await;
}

async fn await_hello(
    socket: &mut WebSocket,
    core: &AudioCore,
    shutdown: &CancellationToken,
) -> bool {
    loop {
        match next_frame(socket, shutdown).await {
            Some(Message::Text(text)) => return finish_hello(socket, core, &text).await,
            Some(Message::Binary(_)) => {
                fail(socket, bad_hello("expected hello")).await;
                return false;
            }
            Some(Message::Ping(_) | Message::Pong(_)) => {}
            Some(Message::Close(_)) | None => return false,
        }
    }
}

async fn finish_hello(socket: &mut WebSocket, core: &AudioCore, text: &str) -> bool {
    let frame = match serde_json::from_str::<AudioMicClientFrame>(text) {
        Ok(frame) => frame,
        Err(error) => {
            tracing::debug!("mic hello rejected: {error}");
            fail(socket, bad_hello("invalid frame")).await;
            return false;
        }
    };
    let AudioMicClientFrame::Hello {
        sample_rate,
        channels,
        format: _,
        v: _,
    } = frame
    else {
        fail(socket, bad_hello("expected hello")).await;
        return false;
    };
    let status = match core.report().await {
        Ok(status) => status,
        Err(error) => {
            fail(socket, fault_frame(&error)).await;
            return false;
        }
    };
    if let Some(message) = hello_mismatch(sample_rate, channels, status.sample_rate) {
        fail(socket, bad_hello(message)).await;
        return false;
    }
    if !status.runtime_ready {
        fail(socket, fault_frame(&AudioFault::NotReady)).await;
        return false;
    }
    emit(
        socket,
        &AudioMicServerFrame::Ready {
            device: status.device,
            sample_rate: status.sample_rate,
            format: AudioFormat::S16Le,
            backend: status.backend,
        },
    )
    .await
}

fn hello_mismatch(sample_rate: Option<u32>, channels: Option<u32>, rate: u32) -> Option<String> {
    if let Some(got) = sample_rate
        && got != rate
    {
        return Some(format!("sample rate must be {rate}"));
    }
    if let Some(got) = channels
        && got != CHANNELS
    {
        return Some(format!("channels must be {CHANNELS}"));
    }
    None
}

async fn run(
    socket: &mut WebSocket,
    core: &AudioCore,
    shutdown: &CancellationToken,
    guard: &mut ArmGuard,
) {
    loop {
        match next_frame(socket, shutdown).await {
            Some(Message::Text(text)) => {
                if !on_text(socket, core, guard, &text).await {
                    return;
                }
            }
            Some(Message::Binary(bytes)) => {
                if !on_binary(socket, core, bytes.as_ref()).await {
                    return;
                }
            }
            Some(Message::Ping(_) | Message::Pong(_)) => {}
            Some(Message::Close(_)) | None => return,
        }
    }
}

async fn on_text(
    socket: &mut WebSocket,
    core: &AudioCore,
    guard: &mut ArmGuard,
    text: &str,
) -> bool {
    let frame = match serde_json::from_str::<AudioMicClientFrame>(text) {
        Ok(frame) => frame,
        Err(error) => {
            tracing::debug!("mic frame rejected: {error}");
            fail(socket, bad_hello("invalid frame")).await;
            return false;
        }
    };
    match frame {
        AudioMicClientFrame::Hello { .. } => {
            fail(socket, bad_hello("hello already complete")).await;
            false
        }
        AudioMicClientFrame::Start => {
            if let Err(error) = core.arm().await {
                fail(socket, fault_frame(&error)).await;
                return false;
            }
            guard.mark();
            true
        }
        AudioMicClientFrame::Stop => {
            if let Err(error) = core.disarm().await {
                fail(socket, fault_frame(&error)).await;
                return false;
            }
            guard.clear();
            emit(socket, &AudioMicServerFrame::Stopped).await
        }
    }
}

async fn on_binary(socket: &mut WebSocket, core: &AudioCore, bytes: &[u8]) -> bool {
    let owned = bytes.to_vec();
    if let Err(error) = core.write(owned).await {
        fail(socket, fault_frame(&error)).await;
        return false;
    }
    true
}

fn bad_hello(message: impl Into<String>) -> AudioMicServerFrame {
    AudioMicServerFrame::Error {
        code: AudioMicErrorCode::BadHello,
        message: message.into(),
    }
}

fn fault_frame(fault: &AudioFault) -> AudioMicServerFrame {
    let code = match fault {
        AudioFault::Busy => AudioMicErrorCode::Busy,
        AudioFault::Alignment | AudioFault::NotArmed => AudioMicErrorCode::BadHello,
        AudioFault::NoReader
        | AudioFault::NotReady
        | AudioFault::Unavailable
        | AudioFault::Escape
        | AudioFault::NotDirectory
        | AudioFault::NotFifo
        | AudioFault::Path
        | AudioFault::Overflow
        | AudioFault::Worker(_)
        | AudioFault::Io(_) => AudioMicErrorCode::NoRuntime,
    };
    AudioMicServerFrame::Error {
        code,
        message: fault.to_string(),
    }
}

async fn next_frame(socket: &mut WebSocket, shutdown: &CancellationToken) -> Option<Message> {
    tokio::select! {
        () = shutdown.cancelled() => {
            close_socket(socket).await;
            None
        }
        incoming = socket.recv() => match incoming {
            Some(Ok(message)) => Some(message),
            Some(Err(error)) => {
                tracing::debug!("mic recv failed: {error}");
                None
            }
            None => None,
        },
    }
}

async fn fail(socket: &mut WebSocket, frame: AudioMicServerFrame) {
    if emit(socket, &frame).await {
        close_socket(socket).await;
    }
}

async fn emit(socket: &mut WebSocket, frame: &AudioMicServerFrame) -> bool {
    let text = match serde_json::to_string(frame) {
        Ok(text) => text,
        Err(error) => {
            tracing::debug!("mic frame encode failed: {error}");
            return false;
        }
    };
    if socket.send(Message::Text(text.into())).await.is_err() {
        tracing::debug!("mic frame was not sent");
        return false;
    }
    true
}

async fn close_socket(socket: &mut WebSocket) {
    if socket.send(Message::Close(None)).await.is_err() {
        tracing::debug!("mic close frame was not sent");
    }
}
