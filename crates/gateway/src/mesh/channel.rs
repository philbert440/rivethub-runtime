use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse as _, Response};
use axum::{Json, Router, routing};
use config::model::MeshTls;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;
use wire::gateway::GatewayError;
use wire::mesh_channel::{AgentList, AgentMessage, AgentReply};

use crate::server::{BindSource, ServeError, ServeOptions, serve_router_with_acceptor};
use crate::tls::{TlsError, TlsPaths, tls_config};

#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("mesh is disabled")]
    Disabled,
    #[error("mesh agent channel requires TLS and a client CA")]
    TlsRequired,
    #[error("mesh.agent_channel_port is required")]
    PortRequired,
    #[error("agent channel requires a positive request timeout")]
    TimeoutRequired,
    #[error("agent channel I/O failed")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Serve(#[from] ServeError),
    #[error(transparent)]
    Tls(#[from] TlsError),
    #[error("TLS configuration worker failed")]
    TlsWorker(#[from] tokio::task::JoinError),
}

#[derive(Debug, Clone)]
pub struct ChannelSettings {
    pub host: String,
    pub agent_channel_port: u16,
    pub tls: TlsPaths,
    pub request_timeout: Duration,
}

impl ChannelSettings {
    pub fn from_config(
        config: &config::Config,
        gateway_tls: TlsPaths,
        request_timeout: Duration,
    ) -> Result<Self, ChannelError> {
        let mesh = config.mesh.as_ref().ok_or(ChannelError::Disabled)?;
        if mesh.enabled != Some(true) {
            return Err(ChannelError::Disabled);
        }
        let tls = match &mesh.tls {
            MeshTls::Enabled(false) => return Err(ChannelError::TlsRequired),
            MeshTls::Enabled(true) => gateway_tls,
            MeshTls::Paths(paths) => TlsPaths {
                cert: paths.cert_path.clone().ok_or(ChannelError::TlsRequired)?,
                key: paths.key_path.clone().ok_or(ChannelError::TlsRequired)?,
                ca: Some(paths.ca_path.clone().ok_or(ChannelError::TlsRequired)?),
            },
        };
        let settings = Self {
            host: BindSource::from_config(config).host,
            agent_channel_port: mesh.agent_channel_port.ok_or(ChannelError::PortRequired)?,
            tls,
            request_timeout,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn validate(&self) -> Result<(), ChannelError> {
        if self
            .tls
            .ca
            .as_ref()
            .is_none_or(|path| path.as_os_str().is_empty())
        {
            return Err(ChannelError::TlsRequired);
        }
        if self.request_timeout.is_zero() {
            return Err(ChannelError::TimeoutRequired);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct AgentCall {
    pub message: AgentMessage,
    pub reply: oneshot::Sender<AgentReply>,
    pub cancellation: CancellationToken,
}

#[derive(Debug)]
struct Dispatch {
    agents: BTreeMap<String, mpsc::Sender<AgentCall>>,
    timeout: Duration,
    shutdown: CancellationToken,
}

#[derive(Debug, Error)]
enum DispatchError {
    #[error("agent mailbox is closed")]
    MailboxClosed,
    #[error("agent stopped before replying")]
    ReplyDropped,
}

#[derive(Debug)]
pub struct AgentChannel {
    listener: TcpListener,
    tls: Arc<rustls::ServerConfig>,
    dispatch: Arc<Dispatch>,
}

impl AgentChannel {
    pub async fn bind(
        settings: ChannelSettings,
        agents: BTreeMap<String, mpsc::Sender<AgentCall>>,
        shutdown: CancellationToken,
    ) -> Result<Self, ChannelError> {
        settings.validate()?;
        let config = tokio::task::spawn_blocking(move || tls_config(&settings.tls)).await??;
        let listener =
            TcpListener::bind((settings.host.as_str(), settings.agent_channel_port)).await?;
        Ok(Self {
            listener,
            tls: Arc::new(config),
            dispatch: Arc::new(Dispatch {
                agents,
                timeout: settings.request_timeout,
                shutdown,
            }),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, ChannelError> {
        Ok(self.listener.local_addr()?)
    }

    pub async fn serve(self) -> Result<(), ChannelError> {
        let shutdown = self.dispatch.shutdown.clone();
        let options = ServeOptions {
            request_timeout: self.dispatch.timeout,
            ..ServeOptions::default()
        };
        let router = Router::new()
            .route("/agents", routing::get(agents))
            .route("/message", routing::post(message))
            .layer(DefaultBodyLimit::max(256 * 1024))
            .with_state(self.dispatch);
        serve_router_with_acceptor(
            self.listener,
            router,
            Some(TlsAcceptor::from(self.tls)),
            shutdown,
            options,
        )
        .await?;
        Ok(())
    }
}

async fn agents(State(state): State<Arc<Dispatch>>) -> Json<AgentList> {
    Json(AgentList {
        agents: state.agents.keys().cloned().collect(),
    })
}

async fn message(
    State(state): State<Arc<Dispatch>>,
    Json(message): Json<AgentMessage>,
) -> Response {
    let Some(agent) = state.agents.get(&message.agent_id) else {
        return failure(StatusCode::NOT_FOUND, "agent not found");
    };
    if message.content.trim().is_empty() {
        return failure(StatusCode::BAD_REQUEST, "content is required");
    }
    let cancellation = state.shutdown.child_token();
    let _cancel_on_drop = cancellation.clone().drop_guard();
    let (reply, received) = oneshot::channel();
    let call = AgentCall {
        message,
        reply,
        cancellation,
    };
    let request = async {
        agent
            .send(call)
            .await
            .map_err(|_| DispatchError::MailboxClosed)?;
        received.await.map_err(|_| DispatchError::ReplyDropped)
    };
    tokio::select! {
        biased;
        () = state.shutdown.cancelled() => failure(StatusCode::SERVICE_UNAVAILABLE, "agent channel is shutting down"),
        result = tokio::time::timeout(state.timeout, request) => match result {
            Ok(Ok(reply)) => Json(reply).into_response(),
            Ok(Err(error)) => {
                tracing::warn!("mesh agent request failed: {error}");
                failure(StatusCode::SERVICE_UNAVAILABLE, "agent unavailable")
            }
            Err(_) => failure(StatusCode::GATEWAY_TIMEOUT, "agent response timed out"),
        }
    }
}

fn failure(status: StatusCode, message: &'static str) -> Response {
    (status, Json(GatewayError::new(message))).into_response()
}
