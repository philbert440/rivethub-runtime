use async_trait::async_trait;
use thiserror::Error;

use crate::BoxError;

#[derive(Debug, Error)]
pub enum HubStateStoreError {
    #[error("hub state store is unavailable")]
    Unavailable(#[source] BoxError),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Versioned<S> {
    pub version: u64,
    pub state: S,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubStateWrite<S> {
    Saved(Versioned<S>),
    Conflict(Versioned<S>),
}

#[async_trait]
pub trait HubStateStore<S>: Send + Sync {
    async fn load(&self, user: &str) -> Result<Versioned<S>, HubStateStoreError>;

    async fn replace(
        &self,
        user: &str,
        base_version: u64,
        state: S,
    ) -> Result<HubStateWrite<S>, HubStateStoreError>;
}
