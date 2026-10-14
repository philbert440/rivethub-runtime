use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::net::UnixStream;
use tokio::sync::broadcast;

use crate::error::Error;
use crate::herdr::CTL_TIMEOUT;
use crate::herdr::PANE_BUSY_RETRY;
use crate::herdr::PANE_READY_TIMEOUT;
use crate::herdr::SEND_TEXT_TIMEOUT;
use crate::herdr::WORKSPACE_CREATE_TIMEOUT;
use crate::herdr::codec::AckError;
use crate::herdr::codec::AckErrorCode;
use crate::herdr::codec::AckFrame;
use crate::herdr::codec::AckResult;
use crate::herdr::codec::AgentStartParams;
use crate::herdr::codec::EventFrame;
use crate::herdr::codec::EventsSubscribeParams;
use crate::herdr::codec::InboundFrame;
use crate::herdr::codec::PaneGetParams;
use crate::herdr::codec::PaneSendTextParams;
use crate::herdr::codec::RequestParams;
use crate::herdr::codec::RpcMethod;
use crate::herdr::codec::WorkspaceCreateParams;
use crate::herdr::codec::decode_line;
use crate::herdr::codec::encode_request;

const EVENT_CAPACITY: usize = 64;

#[derive(Debug, Clone)]
pub struct HerdrClient {
    socket: PathBuf,
    events: broadcast::Sender<EventFrame>,
    next_id: Arc<AtomicU64>,
}

impl HerdrClient {
    #[must_use]
    pub fn new(socket: &Path) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            socket: socket.to_path_buf(),
            events,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<EventFrame> {
        self.events.subscribe()
    }

    pub async fn call(&self, method: RpcMethod, params: RequestParams) -> Result<AckResult, Error> {
        self.call_with_timeout(method, params, call_timeout(method))
            .await
    }

    pub async fn call_with_timeout(
        &self,
        method: RpcMethod,
        params: RequestParams,
        timeout: Duration,
    ) -> Result<AckResult, Error> {
        let id = self.fresh_id();
        let encoded = encode_request(&id, method, &params)?;
        let outcome = tokio::time::timeout(timeout, self.round_trip(method, encoded)).await;
        match outcome {
            Ok(result) => result,
            Err(_) => Err(Error::Rpc {
                method: std::borrow::Cow::Owned(method.to_string()),
                message: format!("timed out after {}ms", timeout.as_millis()),
            }),
        }
    }

    pub async fn agent_start(&self, params: AgentStartParams) -> Result<AckResult, Error> {
        let deadline = tokio::time::Instant::now() + PANE_READY_TIMEOUT;
        loop {
            match self
                .call_with_timeout(
                    RpcMethod::AgentStart,
                    RequestParams::AgentStart(params.clone()),
                    CTL_TIMEOUT,
                )
                .await
            {
                Err(Error::Ack {
                    error:
                        AckError {
                            code: AckErrorCode::AgentPaneBusy,
                            ..
                        },
                    ..
                }) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(PANE_BUSY_RETRY).await;
                }
                other => return other,
            }
        }
    }

    pub async fn pane_send_text(&self, params: PaneSendTextParams) -> Result<AckResult, Error> {
        self.call(RpcMethod::PaneSendText, RequestParams::PaneSendText(params))
            .await
    }

    pub async fn workspace_create(
        &self,
        params: WorkspaceCreateParams,
    ) -> Result<AckResult, Error> {
        self.call(
            RpcMethod::WorkspaceCreate,
            RequestParams::WorkspaceCreate(params),
        )
        .await
    }

    pub async fn pane_get(&self, params: PaneGetParams) -> Result<AckResult, Error> {
        self.call(RpcMethod::PaneGet, RequestParams::PaneGet(params))
            .await
    }

    pub async fn events_subscribe(&self, params: EventsSubscribeParams) -> Result<(), Error> {
        let id = self.fresh_id();
        let encoded = encode_request(
            &id,
            RpcMethod::EventsSubscribe,
            &RequestParams::EventsSubscribe(params),
        )?;
        let stream = self.open().await?;
        let (reader, mut writer) = stream.into_split();
        write_frame(&mut writer, &encoded).await?;
        let mut reader = BufReader::new(reader);
        let ack = self
            .first_ack(RpcMethod::EventsSubscribe, &mut reader)
            .await?;
        match ack_outcome(RpcMethod::EventsSubscribe, ack)? {
            AckResult::SubscriptionStarted => {}
            AckResult::Unrecorded => {
                return Err(Error::Rpc {
                    method: std::borrow::Cow::Borrowed("events.subscribe"),
                    message: "expected subscription_started".to_owned(),
                });
            }
        }
        tokio::spawn(event_pump(self.events.clone(), reader));
        Ok(())
    }

    async fn open(&self) -> Result<UnixStream, Error> {
        Ok(UnixStream::connect(&self.socket).await?)
    }

    async fn round_trip(&self, method: RpcMethod, encoded: Vec<u8>) -> Result<AckResult, Error> {
        let stream = self.open().await?;
        let (reader, mut writer) = stream.into_split();
        write_frame(&mut writer, &encoded).await?;
        let mut reader = BufReader::new(reader);
        let ack = self.first_ack(method, &mut reader).await?;
        ack_outcome(method, ack)
    }

    async fn first_ack(
        &self,
        method: RpcMethod,
        reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    ) -> Result<AckFrame, Error> {
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line).await?;
            if read == 0 {
                return Err(closed(method));
            }
            let trimmed: &[u8] = trim_newline(&line);
            if trimmed.is_empty() {
                continue;
            }
            match decode_line(trimmed) {
                Ok(InboundFrame::Ack(ack)) => return Ok(ack),
                Ok(InboundFrame::Event(event)) => {
                    drop(self.events.send(event));
                }
                Err(_) => {}
            }
        }
    }

    fn fresh_id(&self) -> String {
        self.next_id.fetch_add(1, Ordering::Relaxed).to_string()
    }
}

async fn write_frame(
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    encoded: &[u8],
) -> Result<(), Error> {
    writer.write_all(encoded).await?;
    writer.flush().await?;
    Ok(())
}

async fn event_pump(
    events: broadcast::Sender<EventFrame>,
    mut reader: BufReader<tokio::net::unix::OwnedReadHalf>,
) {
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line).await;
        match read {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let trimmed: &[u8] = trim_newline(&line);
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(InboundFrame::Event(event)) = decode_line(trimmed)
                    && events.send(event).is_err()
                {
                    break;
                }
            }
        }
    }
}

fn call_timeout(method: RpcMethod) -> Duration {
    match method {
        RpcMethod::WorkspaceCreate => WORKSPACE_CREATE_TIMEOUT,
        RpcMethod::PaneSendText => SEND_TEXT_TIMEOUT,
        RpcMethod::AgentStart | RpcMethod::PaneGet | RpcMethod::EventsSubscribe => CTL_TIMEOUT,
    }
}

fn closed(method: RpcMethod) -> Error {
    Error::Rpc {
        method: std::borrow::Cow::Owned(method.to_string()),
        message: "socket closed".to_owned(),
    }
}

fn ack_outcome(method: RpcMethod, ack: AckFrame) -> Result<AckResult, Error> {
    match ack.error {
        Some(error) => Err(Error::Ack { method, error }),
        None => Ok(ack.result.unwrap_or(AckResult::Unrecorded)),
    }
}

fn trim_newline(line: &[u8]) -> &[u8] {
    let mut trimmed = line;
    while let Some(rest) = trimmed
        .strip_suffix(b"\n")
        .or_else(|| trimmed.strip_suffix(b"\r"))
    {
        trimmed = rest;
    }
    trimmed
}
