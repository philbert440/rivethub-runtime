#![cfg(test)]

#[path = "../../../testkit/tests/support/task_spec.rs"]
mod contract;

use std::path::PathBuf;

use contract::{RawStore, StoredRow};
use kernel::memory::MemoryStore as _;
use kernel::tasks::TaskStore;
use memory_sqlite::SqliteStore;
use rusqlite::params;
use uuid::Uuid;
use wire::JsonObject;

struct Rows(PathBuf);

fn json_text<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("json")
}

#[async_trait::async_trait]
impl RawStore for Rows {
    async fn insert_row(&self, row: &StoredRow) {
        let conn = rusqlite::Connection::open(&self.0).expect("fixture connection");
        conn.execute(
            "INSERT INTO rh_tasks (
                id, goal, context_refs, acceptance_criteria, spec, executor, executor_target,
                requested_by, agent_id, origin, parent_task_id, chain_depth, budget, node_affinity,
                usage, status, attempt, max_attempts, result, error, conversation_id, session_key,
                pending_message, claimed_by, harness_session_ids, created_at, started_at,
                last_heartbeat_at, completed_at, duration_ms, eval, eval_attempt
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32
             )",
            params![
                row.id.to_string(),
                row.goal,
                json_text(&row.context_refs),
                json_text(&row.acceptance_criteria),
                json_text(&row.spec),
                row.executor.to_string(),
                row.executor_target,
                row.requested_by,
                row.agent_id,
                row.origin.to_string(),
                row.parent_task_id.map(|id| id.to_string()),
                row.chain_depth,
                json_text(&row.budget),
                row.node_affinity,
                row.usage.as_ref().map(json_text),
                row.status.to_string(),
                row.attempt,
                row.max_attempts,
                row.result.as_ref().map(json_text),
                row.error,
                row.conversation_id.map(|id| id.to_string()),
                row.session_key,
                row.pending_message,
                row.claimed_by,
                json_text(&row.harness_session_ids),
                row.created_at.to_rfc3339(),
                row.started_at.map(|at| at.to_rfc3339()),
                row.last_heartbeat_at.map(|at| at.to_rfc3339()),
                row.completed_at.map(|at| at.to_rfc3339()),
                row.duration_ms,
                row.eval.as_ref().map(json_text),
                row.eval_attempt,
            ],
        )
        .expect("insert fixture row");
    }

    async fn raw_spec(&self, id: Uuid) -> JsonObject {
        let conn = rusqlite::Connection::open(&self.0).expect("fixture connection");
        let spec: String = conn
            .query_row(
                "SELECT spec FROM rh_tasks WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .expect("stored bag");
        serde_json::from_str(&spec).expect("bag object")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn task_spec_contract() {
    let dir = tempfile::tempdir().expect("temporary database");
    let path = dir.path().join("memory.sqlite");
    migrate::sqlite(&path).expect("migrate");
    let store = SqliteStore::open(&path).expect("store");
    let peer = SqliteStore::open(&path).expect("independent connection");
    let missing = Uuid::now_v7();
    assert!(
        matches!(store.load(missing).await, Err(kernel::tasks::TaskError::Missing { id }) if id == missing)
    );
    contract::run(&store, &peer, &Rows(path)).await;
    store.close().await.expect("close");
    peer.close().await.expect("close peer");
}
