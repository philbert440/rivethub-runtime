use async_trait::async_trait;
use thiserror::Error;
use wire::gateway::hub_state::HubState;
use wire::gateway::hub_state::HubStateResponse;

use crate::BoxError;

#[derive(Debug, Error)]
pub enum HubStateStoreError {
    #[error("hub state store is unavailable")]
    Unavailable(#[source] BoxError),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HubStateWrite {
    Saved(HubStateResponse),
    Conflict(HubStateResponse),
}

#[async_trait]
pub trait HubStateStore: Send + Sync {
    async fn load(&self, user: &str) -> Result<HubStateResponse, HubStateStoreError>;

    async fn replace(
        &self,
        user: &str,
        base_version: u64,
        state: HubState,
    ) -> Result<HubStateWrite, HubStateStoreError>;
}
