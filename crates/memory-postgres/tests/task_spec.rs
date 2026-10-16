#![cfg(test)]

#[path = "../../testkit/tests/support/task_spec.rs"]
mod contract;

use contract::{RawStore, StoredRow};
use memory_postgres::PostgresTasks;
use sqlx::PgPool;
use uuid::Uuid;
use wire::JsonObject;

struct Rows(PgPool);

fn json_text<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("json")
}

const INSERT_ROW: &str = concat!(
    "INSERT INTO rh_tasks (",
    "id, goal, context_refs, acceptance_criteria, spec, executor, executor_target, ",
    "requested_by, agent_id, origin, parent_task_id, chain_depth, budget, node_affinity, ",
    "usage, status, attempt, max_attempts, result, error, conversation_id, session_key, ",
    "pending_message, claimed_by, harness_session_ids, created_at, started_at, ",
    "last_heartbeat_at, completed_at, duration_ms, eval, eval_attempt",
    ") VALUES (",
    "$1, $2, $3::jsonb, $4::jsonb, $5::jsonb, $6, $7, ",
    "$8, $9, $10, $11, $12, $13::jsonb, $14, ",
    "$15::jsonb, $16, $17, $18, $19::jsonb, $20, $21, $22, ",
    "$23, $24, $25::jsonb, $26, $27, ",
    "$28, $29, $30, $31::jsonb, $32)"
);

#[async_trait::async_trait]
impl RawStore for Rows {
    async fn insert_row(&self, row: &StoredRow) {
        sqlx::query(INSERT_ROW)
            .bind(row.id)
            .bind(&row.goal)
            .bind(json_text(&row.context_refs))
            .bind(json_text(&row.acceptance_criteria))
            .bind(json_text(&row.spec))
            .bind(row.executor.to_string())
            .bind(&row.executor_target)
            .bind(&row.requested_by)
            .bind(&row.agent_id)
            .bind(row.origin.to_string())
            .bind(row.parent_task_id)
            .bind(row.chain_depth)
            .bind(json_text(&row.budget))
            .bind(&row.node_affinity)
            .bind(row.usage.as_ref().map(json_text))
            .bind(row.status.to_string())
            .bind(row.attempt)
            .bind(row.max_attempts)
            .bind(row.result.as_ref().map(json_text))
            .bind(&row.error)
            .bind(row.conversation_id)
            .bind(&row.session_key)
            .bind(&row.pending_message)
            .bind(&row.claimed_by)
            .bind(json_text(&row.harness_session_ids))
            .bind(row.created_at)
            .bind(row.started_at)
            .bind(row.last_heartbeat_at)
            .bind(row.completed_at)
            .bind(row.duration_ms)
            .bind(row.eval.as_ref().map(json_text))
            .bind(row.eval_attempt)
            .execute(&self.0)
            .await
            .expect("insert row");
    }
    async fn raw_spec(&self, id: Uuid) -> JsonObject {
        let spec: sqlx::types::Json<JsonObject> =
            sqlx::query_scalar("SELECT spec FROM rh_tasks WHERE id = $1")
                .bind(id)
                .fetch_one(&self.0)
                .await
                .expect("spec");
        spec.0
    }
}

#[sqlx::test(migrator = "migrate::POSTGRES")]
async fn task_spec_contract(pool: PgPool) {
    let store = PostgresTasks::from_pool(pool.clone());
    let peer = PostgresTasks::from_pool(pool.clone());
    contract::run(&store, &peer, &Rows(pool)).await;
}
