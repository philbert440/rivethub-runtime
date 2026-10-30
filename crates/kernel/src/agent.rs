use std::collections::BTreeSet;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use values::Message;
use wire::hooks::{HookContextBase, HookUsage};

use crate::model::{ModelChunk, ModelRequest};
use crate::{BoxError, ToolContext};

#[derive(Debug, Clone, Copy)]
pub struct TurnLimits {
    pub steps: u32,
    pub timeout: Duration,
    pub cleanup_timeout: Duration,
    pub response_bytes: usize,
}

#[derive(Debug)]
pub struct Turn {
    pub request: ModelRequest,
    pub context: HookContextBase,
    pub tool_context: ToolContext,
    pub authorized_tools: BTreeSet<String>,
    pub excluded_tools: BTreeSet<String>,
    pub limits: TurnLimits,
}

#[must_use]
pub fn tool_permitted(
    name: &str,
    authorized: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
) -> bool {
    authorized.contains(name) && !excluded.contains(name)
}

#[must_use]
pub fn excluded_names(names: Option<&[String]>) -> BTreeSet<String> {
    match names {
        Some(names) => names.iter().cloned().collect(),
        None => BTreeSet::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    Completed,
    StepLimit,
}

#[derive(Debug, PartialEq, Eq)]
pub struct TurnResult {
    pub messages: Vec<Message>,
    pub response: String,
    pub tools_used: Vec<String>,
    pub iterations: u32,
    pub usage: Option<HookUsage>,
    pub stop: StopReason,
}

#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    #[error("turn cancelled")]
    Cancelled,
    #[error("turn timed out")]
    Timeout,
    #[error("turn failed: {0}")]
    Failed(#[source] BoxError),
}

#[async_trait]
pub trait TurnRunner: Send + Sync {
    async fn run(
        &self,
        turn: Turn,
        events: mpsc::Sender<ModelChunk>,
        cancel: CancellationToken,
    ) -> Result<TurnResult, TurnError>;
}
