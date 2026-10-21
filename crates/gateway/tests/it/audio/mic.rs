use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use wire::events::ProtocolVersion;
use wire::gateway::devices::{
    AudioBackend, AudioFormat, AudioMicClientFrame, AudioMicErrorCode, AudioMicServerFrame,
};

use super::{AudioLive, MicSocket, audio_status, call};

const WAIT: Duration = Duration::from_secs(30);

pub(super) async fn round_trip(live: &AudioLive) -> Result<(), String> {
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    ready(&mut socket).await?;
    let reader = open_reader(&live.dir.join("mic.pcm"))?;
    start_publisher(&mut socket).await?;
    let armed = audio_status(
        &call(
            &live.running.gateway.router(false),
            "GET",
            "/audio/status",
            Vec::new(),
            &[],
        )
        .await?,
    )?;
    assert!(armed.armed);
    assert!(armed.publisher_id.is_some());
    assert_eq!(armed.bytes_written, 0);
    send_binary(&mut socket, &[1, 2, 3, 4]).await?;
    send_text(&mut socket, r#"{"type":"stop"}"#).await?;
    assert_eq!(
        recv_server(&mut socket, "stopped").await?,
        AudioMicServerFrame::Stopped
    );
    assert_eq!(read_pcm(reader)?, vec![1, 2, 3, 4]);
    let done = audio_status(
        &call(
            &live.running.gateway.router(false),
            "GET",
            "/audio",
            Vec::new(),
            &[],
        )
        .await?,
    )?;
    assert!(!done.armed);
    assert!(done.publisher_id.is_none());
    assert_eq!(done.bytes_written, 4);
    Ok(())
}

pub(super) async fn reject_before_arm(live: &AudioLive) -> Result<(), String> {
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    send_text(&mut socket, r#"{"type":"start"}"#).await?;
    expect_fault(&mut socket, AudioMicErrorCode::BadHello, "expected hello").await?;
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    let slow = hello(Some(8_000), Some(1))?;
    send_text(&mut socket, &slow).await?;
    expect_fault(
        &mut socket,
        AudioMicErrorCode::BadHello,
        "sample rate must be 16000",
    )
    .await?;
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    let wide = hello(Some(16_000), Some(2))?;
    send_text(&mut socket, &wide).await?;
    expect_fault(
        &mut socket,
        AudioMicErrorCode::BadHello,
        "channels must be 1",
    )
    .await?;
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    ready(&mut socket).await?;
    send_binary(&mut socket, &[9, 8]).await?;
    expect_fault(
        &mut socket,
        AudioMicErrorCode::BadHello,
        "binary audio requires start",
    )
    .await?;
    Ok(())
}

pub(super) async fn reject_odd_pcm(live: &AudioLive) -> Result<(), String> {
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    ready(&mut socket).await?;
    let _reader = open_reader(&live.dir.join("mic.pcm"))?;
    send_text(&mut socket, r#"{"type":"start"}"#).await?;
    send_binary(&mut socket, &[1]).await?;
    expect_fault(
        &mut socket,
        AudioMicErrorCode::BadHello,
        "pcm length must be even",
    )
    .await?;
    Ok(())
}

pub(super) async fn second_publisher_is_busy(live: &AudioLive) -> Result<(), String> {
    let mut first = connect(&live.running.addr, "/audio/mic").await?;
    let mut second = connect(&live.running.addr, "/api/audio/mic").await?;
    ready(&mut first).await?;
    ready(&mut second).await?;
    let _reader = open_reader(&live.dir.join("mic.pcm"))?;
    start_publisher(&mut first).await?;
    send_text(&mut second, r#"{"type":"start"}"#).await?;
    expect_fault(
        &mut second,
        AudioMicErrorCode::Busy,
        "another publisher is armed",
    )
    .await?;
    send_text(&mut first, r#"{"type":"start"}"#).await?;
    expect_fault(
        &mut first,
        AudioMicErrorCode::Busy,
        "another publisher is armed",
    )
    .await?;
    Ok(())
}

pub(super) async fn shutdown_sends_close(live: &AudioLive) -> Result<(), String> {
    let mut socket = connect(&live.running.addr, "/audio/mic").await?;
    ready(&mut socket).await?;
    live.running.token.cancel();
    match next_message(&mut socket, "shutdown close").await? {
        Message::Close(_) => Ok(()),
        Message::Text(text) => Err(format!("text during shutdown: {text}")),
        Message::Binary(_) => Err("binary during shutdown".to_owned()),
        Message::Ping(_) => Err("ping during shutdown".to_owned()),
        Message::Pong(_) => Err("pong during shutdown".to_owned()),
        Message::Frame(_) => Err("frame during shutdown".to_owned()),
    }
}

async fn ready(socket: &mut MicSocket) -> Result<(), String> {
    let frame = hello(Some(16_000), Some(1))?;
    send_text(socket, &frame).await?;
    match recv_server(socket, "ready").await? {
        AudioMicServerFrame::Ready {
            device,
            sample_rate,
            format,
            backend,
        } => {
            assert_eq!(device, "RivetHub Mic");
            assert_eq!(sample_rate, 16_000);
            assert_eq!(format, AudioFormat::S16Le);
            assert_eq!(backend, AudioBackend::FifoShim);
            Ok(())
        }
        AudioMicServerFrame::Error { code, message } => {
            Err(format!("hello failed: {code:?} {message}"))
        }
        AudioMicServerFrame::Stopped => Err("stopped before ready".to_owned()),
    }
}

async fn start_publisher(socket: &mut MicSocket) -> Result<(), String> {
    send_text(socket, r#"{"type":"start"}"#).await?;
    let marker = b"publisher armed".as_slice();
    socket
        .send(Message::Ping(marker.to_vec().into()))
        .await
        .map_err(|error| error.to_string())?;
    let message = next_message(socket, "publisher arm barrier").await?;
    assert_eq!(message, Message::Pong(marker.to_vec().into()));
    Ok(())
}

pub(super) async fn expect_fault(
    socket: &mut MicSocket,
    code: AudioMicErrorCode,
    message: &str,
) -> Result<(), String> {
    match recv_server(socket, &format!("fault:{message}")).await? {
        AudioMicServerFrame::Error {
            code: got,
            message: text,
        } => {
            assert_eq!(got, code);
            assert_eq!(text, message);
        }
        AudioMicServerFrame::Ready { .. } => return Err("ready".to_owned()),
        AudioMicServerFrame::Stopped => return Err("stopped".to_owned()),
    }
    match next_message(socket, "fault close").await? {
        Message::Close(_) => Ok(()),
        Message::Text(text) => Err(format!("text after error: {text}")),
        Message::Binary(_) => Err("binary after error".to_owned()),
        Message::Ping(_) => Err("ping after error".to_owned()),
        Message::Pong(_) => Err("pong after error".to_owned()),
        Message::Frame(_) => Err("frame after error".to_owned()),
    }
}

pub(super) fn hello(sample_rate: Option<u32>, channels: Option<u32>) -> Result<String, String> {
    serde_json::to_string(&AudioMicClientFrame::Hello {
        v: ProtocolVersion::V1,
        sample_rate,
        channels,
        format: Some(AudioFormat::S16Le),
    })
    .map_err(|error| error.to_string())
}

pub(super) async fn send_text(socket: &mut MicSocket, text: &str) -> Result<(), String> {
    socket
        .send(Message::text(text))
        .await
        .map_err(|error| error.to_string())
}

async fn send_binary(socket: &mut MicSocket, bytes: &[u8]) -> Result<(), String> {
    socket
        .send(Message::binary(bytes.to_vec()))
        .await
        .map_err(|error| error.to_string())
}

async fn recv_server(socket: &mut MicSocket, step: &str) -> Result<AudioMicServerFrame, String> {
    match next_message(socket, step).await? {
        Message::Text(text) => serde_json::from_str(&text).map_err(|error| error.to_string()),
        Message::Binary(_) => Err("binary from the mic bridge".to_owned()),
        Message::Ping(_) => Err("ping from the mic bridge".to_owned()),
        Message::Pong(_) => Err("pong from the mic bridge".to_owned()),
        Message::Close(_) => Err("closed before a frame".to_owned()),
        Message::Frame(_) => Err("raw frame from the mic bridge".to_owned()),
    }
}

async fn next_message(socket: &mut MicSocket, step: &str) -> Result<Message, String> {
    match tokio::time::timeout(WAIT, socket.next()).await {
        Ok(Some(Ok(message))) => Ok(message),
        Ok(Some(Err(error))) => Err(format!("{step}: {error}")),
        Ok(None) => Err(format!("{step}: socket closed")),
        Err(error) => Err(format!("{step}: {error}")),
    }
}

pub(super) async fn connect(addr: &SocketAddr, path: &str) -> Result<MicSocket, String> {
    let (socket, _) = tokio::time::timeout(
        WAIT,
        tokio_tungstenite::connect_async(format!("ws://{addr}{path}")),
    )
    .await
    .map_err(|error| format!("mic connect: {error}"))?
    .map_err(|error| format!("mic connect: {error}"))?;
    Ok(socket)
}

pub(super) async fn destroyed(addr: &SocketAddr, path: &str) -> Result<Vec<u8>, String> {
    let mut stream = TcpStream::connect(*addr)
        .await
        .map_err(|error| error.to_string())?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    let mut buf = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut buf))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    Ok(buf)
}

fn open_reader(path: &Path) -> Result<std::fs::File, String> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| error.to_string())
}

fn read_pcm(mut file: std::fs::File) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut out = Vec::new();
    let mut buf = [0u8; 64];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(count) => {
                let Some(part) = buf.get(..count) else {
                    return Err("fifo read was longer than the buffer".to_owned());
                };
                out.extend_from_slice(part);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(out)
}
