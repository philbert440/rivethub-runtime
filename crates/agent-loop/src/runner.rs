use async_trait::async_trait;
use kernel::agent::{Turn, TurnError, TurnResult, TurnRunner};
use kernel::model::ModelChunk;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{AgentLoop, Error};

#[async_trait]
impl TurnRunner for AgentLoop {
    async fn run(
        &self,
        turn: Turn,
        events: mpsc::Sender<ModelChunk>,
        cancel: CancellationToken,
    ) -> Result<TurnResult, TurnError> {
        Self::run(self, turn, events, cancel)
            .await
            .map_err(|error| match error {
                Error::Cancelled => TurnError::Cancelled,
                Error::Timeout => TurnError::Timeout,
                error @ (Error::Hook(_)
                | Error::Provider(_)
                | Error::Json(_)
                | Error::Task(_)
                | Error::Stream(_)
                | Error::Refused
                | Error::Disconnected
                | Error::DuplicateTool(_)
                | Error::MissingUserMessage
                | Error::UsageOverflow
                | Error::DurationOverflow) => TurnError::Failed(Box::new(error)),
            })
    }
}
