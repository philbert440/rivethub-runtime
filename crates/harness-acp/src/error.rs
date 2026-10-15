use std::fmt;

use thiserror::Error;
use wire::harness::{ApprovalDecision, HarnessId};

use crate::identity::IdentityError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatUnsupportedReason {
    TerminalOnly,
    CaptureOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnverifiedReason {
    NotInstalled,
}

impl fmt::Display for ChatUnsupportedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TerminalOnly => f.write_str(
                "it is terminal-only and has no ACP endpoint; chat runs through the terminal view",
            ),
            Self::CaptureOnly => f.write_str("it is capture-only and cannot run chat turns"),
        }
    }
}

impl fmt::Display for UnverifiedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => f.write_str("not installed"),
        }
    }
}

#[derive(Debug, Error)]
pub enum AcpError {
    #[error("{harness} does not support chat over ACP: {reason}")]
    ChatUnsupported {
        harness: HarnessId,
        reason: ChatUnsupportedReason,
    },
    #[error("failed to launch {program}: {source}")]
    Launch {
        program: String,
        source: std::io::Error,
    },
    #[error("{program} did not open its stdio pipes")]
    Stdio { program: String },
    #[error("ACP {method} failed: {source}")]
    Protocol {
        method: &'static str,
        source: agent_client_protocol::Error,
    },
    #[error("the agent behind {method} is gone")]
    Closed { method: &'static str },
    #[error("session {session} is already active")]
    SessionActive {
        session: wire::session_id::SessionId,
    },
    #[error("no ACP launch is configured for {harness}")]
    Unconfigured { harness: HarnessId },
    #[error("the ACP invocation of {program} for {harness} is not verified: {reason}")]
    UnverifiedLaunch {
        harness: HarnessId,
        program: String,
        reason: UnverifiedReason,
    },
    #[error("the agent does not offer a {category} config option")]
    ConfigOptionUnavailable { category: &'static str },
    #[error("the agent rejected {category} value {value}")]
    ConfigValueRejected {
        category: &'static str,
        value: String,
    },
    #[error("the permission request has no tool title")]
    MissingPermissionTitle,
    #[error("ACP media content cannot be represented as text")]
    NonTextContent,
    #[error("ACP discovery timed out")]
    DiscoveryTimeout,
    #[error("the session update has no kernel event")]
    UnmappedUpdate,
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error("no permission request {request_id} is waiting for an answer")]
    UnknownPermission { request_id: String },
    #[error("the agent offered no option to {decision:?} this permission request")]
    UnanswerablePermission { decision: ApprovalDecision },
    #[error("the agent reported a tool input that is not a JSON object: {source}")]
    ToolInput { source: serde_json::Error },
    #[error(
        "no working directory: the request, the launch command and runtime.workspace all leave it unset"
    )]
    WorkingDir,
}

pub(crate) fn port(error: impl Into<AcpError>) -> kernel::harness::HarnessAdapterError {
    let error: AcpError = error.into();
    kernel::harness::HarnessAdapterError::Failed {
        message: error.to_string(),
        source: Box::new(error),
    }
}
