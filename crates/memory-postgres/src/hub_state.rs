use async_trait::async_trait;
use kernel::hub_state::HubStateStore;
use kernel::hub_state::HubStateStoreError;
use kernel::hub_state::HubStateWrite;
use sqlx::PgPool;
use sqlx::types::Json;
use wire::gateway::hub_state::HubState;
use wire::gateway::hub_state::HubStateResponse;

use crate::Error;
use crate::PostgresMemory;

const LOAD: &str = "SELECT version, state FROM rh_hub_state WHERE user_id = $1";

const CREATE: &str = "INSERT INTO rh_hub_state (user_id, version, state) VALUES ($1, 1, $2)
     ON CONFLICT (user_id) DO NOTHING
     RETURNING version, state";

const ADVANCE: &str =
    "UPDATE rh_hub_state SET version = version + 1, state = $3, updated_at = now()
     WHERE user_id = $1 AND version = $2
     RETURNING version, state";

#[derive(Debug, thiserror::Error)]
#[error("hub state version {0} is out of range")]
struct VersionRange(i64);

#[derive(Debug, sqlx::FromRow)]
struct StoredRow {
    version: i64,
    state: Json<HubState>,
}

#[derive(Debug, Clone)]
pub struct PostgresHubState {
    pool: PgPool,
}

impl PostgresHubState {
    pub async fn detect(pool: PgPool) -> Result<Option<Self>, Error> {
        let present =
            sqlx::query_scalar::<_, bool>("SELECT to_regclass('rh_hub_state') IS NOT NULL")
                .fetch_one(&pool)
                .await
                .map_err(|source| Error::operation("hub state table probe", source))?;
        Ok(present.then_some(Self { pool }))
    }

    async fn current(&self, user: &str) -> Result<HubStateResponse, HubStateStoreError> {
        let row = sqlx::query_as::<_, StoredRow>(LOAD)
            .bind(user)
            .fetch_optional(&self.pool)
            .await
            .map_err(|source| failed("hub state load", source))?;
        row.map_or_else(|| Ok(HubStateResponse::default()), stored)
    }
}

impl PostgresMemory {
    pub async fn hub_state(&self) -> Result<Option<PostgresHubState>, Error> {
        if self.unverified {
            return Ok(Some(PostgresHubState {
                pool: self.pool.clone(),
            }));
        }
        PostgresHubState::detect(self.pool.clone()).await
    }
}

#[async_trait]
impl HubStateStore for PostgresHubState {
    async fn load(&self, user: &str) -> Result<HubStateResponse, HubStateStoreError> {
        self.current(user).await
    }

    async fn replace(
        &self,
        user: &str,
        base_version: u64,
        state: HubState,
    ) -> Result<HubStateWrite, HubStateStoreError> {
        let base = i64::try_from(base_version)
            .map_err(|error| HubStateStoreError::Unavailable(Box::new(error)))?;
        let query = if base == 0 {
            sqlx::query_as::<_, StoredRow>(CREATE)
                .bind(user)
                .bind(Json(&state))
        } else {
            sqlx::query_as::<_, StoredRow>(ADVANCE)
                .bind(user)
                .bind(base)
                .bind(Json(&state))
        };
        let written = query
            .fetch_optional(&self.pool)
            .await
            .map_err(|source| failed("hub state write", source))?;
        match written {
            Some(row) => Ok(HubStateWrite::Saved(stored(row)?)),
            None => Ok(HubStateWrite::Conflict(self.current(user).await?)),
        }
    }
}

fn stored(row: StoredRow) -> Result<HubStateResponse, HubStateStoreError> {
    let version = u64::try_from(row.version)
        .map_err(|_| HubStateStoreError::Unavailable(Box::new(VersionRange(row.version))))?;
    Ok(HubStateResponse {
        version,
        state: row.state.0,
    })
}

fn failed(operation: &'static str, source: sqlx::Error) -> HubStateStoreError {
    HubStateStoreError::Unavailable(Box::new(Error::operation(operation, source)))
}
