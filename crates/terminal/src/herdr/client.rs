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
use crate::herdr::codec::AckResultKind;
use crate::herdr::codec::AgentStartParams;
use crate::herdr::codec::EventFrame;
use crate::herdr::codec::EventsSubscribeParams;
use crate::herdr::codec::InboundFrame;
use crate::herdr::codec::PaneGetParams;
use crate::herdr::codec::PaneSendTextParams;
use crate::herdr::codec::RequestFrame;
use crate::herdr::codec::RequestParams;
use crate::herdr::codec::RpcMethod;
use crate::herdr::codec::WorkspaceCreateParams;
use crate::herdr::codec::decode_line;

const EVENT_CAPACITY: usize = 64;

#[derive(Debug)]
struct Lifetime {
    shutdown: tokio_util::sync::CancellationToken,
    tasks: tokio_util::task::TaskTracker,
}

impl Drop for Lifetime {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.tasks.close();
    }
}

#[derive(Debug, Clone)]
pub struct HerdrClient {
    socket: PathBuf,
    events: broadcast::Sender<Result<EventFrame, Arc<Error>>>,
    next_id: Arc<AtomicU64>,
    lifetime: Arc<Lifetime>,
}

impl HerdrClient {
    #[must_use]
    pub fn new(socket: &Path) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            socket: socket.to_path_buf(),
            events,
            next_id: Arc::new(AtomicU64::new(1)),
            lifetime: Arc::new(Lifetime {
                shutdown: tokio_util::sync::CancellationToken::new(),
                tasks: tokio_util::task::TaskTracker::new(),
            }),
        }
    }

    #[must_use]
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<Result<EventFrame, Arc<Error>>> {
        self.events.subscribe()
    }

    pub async fn call(
        &self,
        method: RpcMethod,
        params: RequestParams,
    ) -> Result<Option<AckResult>, Error> {
        self.call_with_timeout(method, params, call_timeout(method))
            .await
    }

    pub async fn call_with_timeout(
        &self,
        method: RpcMethod,
        params: RequestParams,
        timeout: Duration,
    ) -> Result<Option<AckResult>, Error> {
        let id = self.fresh_id();
        let encoded = RequestFrame {
            id: id.clone(),
            method,
            params,
        }
        .encode()?;
        let outcome = tokio::time::timeout(timeout, self.round_trip(method, &id, encoded)).await;
        match outcome {
            Ok(result) => result,
            Err(_) => Err(Error::RpcTimeout {
                method,
                timeout_ms: timeout.as_millis(),
            }),
        }
    }

    pub async fn agent_start(&self, params: AgentStartParams) -> Result<Option<AckResult>, Error> {
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
                            code: Some(AckErrorCode::AgentPaneBusy),
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

    pub async fn pane_send_text(
        &self,
        params: PaneSendTextParams,
    ) -> Result<Option<AckResult>, Error> {
        self.call(RpcMethod::PaneSendText, RequestParams::PaneSendText(params))
            .await
    }

    pub async fn workspace_create(
        &self,
        params: WorkspaceCreateParams,
    ) -> Result<Option<AckResult>, Error> {
        self.call(
            RpcMethod::WorkspaceCreate,
            RequestParams::WorkspaceCreate(params),
        )
        .await
    }

    pub async fn pane_get(&self, params: PaneGetParams) -> Result<Option<AckResult>, Error> {
        self.call(RpcMethod::PaneGet, RequestParams::PaneGet(params))
            .await
    }

    pub async fn events_subscribe(&self, params: EventsSubscribeParams) -> Result<(), Error> {
        let id = self.fresh_id();
        let encoded = RequestFrame {
            id: id.clone(),
            method: RpcMethod::EventsSubscribe,
            params: RequestParams::EventsSubscribe(params),
        }
        .encode()?;
        let handshake = async {
            let mut stream = BufReader::new(self.open().await?);
            stream.get_mut().write_all(&encoded).await?;
            stream.get_mut().flush().await?;
            let ack = self
                .first_ack(RpcMethod::EventsSubscribe, &id, &mut stream)
                .await?;
            Ok::<_, Error>((stream, ack))
        };
        let (stream, ack) = tokio::time::timeout(CTL_TIMEOUT, handshake)
            .await
            .map_err(|_| Error::RpcTimeout {
                method: RpcMethod::EventsSubscribe,
                timeout_ms: CTL_TIMEOUT.as_millis(),
            })??;
        if !ack_outcome(RpcMethod::EventsSubscribe, ack)?
            .is_some_and(|result| result.kind == AckResultKind::SubscriptionStarted)
        {
            return Err(Error::SubscriptionResult);
        }
        let events = self.events.clone();
        let shutdown = self.lifetime.shutdown.clone();
        self.lifetime.tasks.spawn(async move {
            let result = tokio::select! {
                () = shutdown.cancelled() => return,
                result = event_pump(&events, stream) => result,
            };
            if let Err(error) = result {
                drop(events.send(Err(Arc::new(error))));
            }
        });
        Ok(())
    }

    async fn open(&self) -> Result<UnixStream, Error> {
        Ok(UnixStream::connect(&self.socket).await?)
    }

    async fn round_trip(
        &self,
        method: RpcMethod,
        id: &str,
        encoded: Vec<u8>,
    ) -> Result<Option<AckResult>, Error> {
        let stream = self.open().await?;
        let (reader, mut writer) = stream.into_split();
        write_frame(&mut writer, &encoded).await?;
        let mut reader = BufReader::new(reader);
        let ack = self.first_ack(method, id, &mut reader).await?;
        ack_outcome(method, ack)
    }

    async fn first_ack(
        &self,
        method: RpcMethod,
        id: &str,
        reader: &mut (impl tokio::io::AsyncBufRead + Unpin),
    ) -> Result<AckFrame, Error> {
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line).await?;
            if read == 0 {
                return Err(Error::RpcClosed { method });
            }
            let trimmed: &[u8] = trim_newline(&line);
            if trimmed.is_empty() {
                continue;
            }
            match decode_line(trimmed)? {
                InboundFrame::Ack(ack) => {
                    if ack.id != id {
                        return Err(Error::AckId {
                            expected: id.to_owned(),
                            actual: ack.id,
                        });
                    }
                    return Ok(ack);
                }
                InboundFrame::Event(event) => {
                    drop(self.events.send(Ok(event)));
                }
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
    events: &broadcast::Sender<Result<EventFrame, Arc<Error>>>,
    mut reader: BufReader<UnixStream>,
) -> Result<(), Error> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).await? == 0 {
            return Err(Error::RpcClosed {
                method: RpcMethod::EventsSubscribe,
            });
        }
        let trimmed = trim_newline(&line);
        if trimmed.is_empty() {
            continue;
        }
        match decode_line(trimmed)? {
            InboundFrame::Event(event) => {
                drop(events.send(Ok(event)));
            }
            InboundFrame::Ack(_) => return Err(Error::SubscriptionResult),
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

fn ack_outcome(method: RpcMethod, ack: AckFrame) -> Result<Option<AckResult>, Error> {
    match ack.error {
        Some(error) => Err(Error::Ack { method, error }),
        None => Ok(ack.result),
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
