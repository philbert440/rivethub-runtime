use std::sync::Arc;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::model::ModelProvider;
use kernel::model::ModelProviderError;
use parking_lot::Mutex;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use serde::Deserialize;
use serde::Serialize;
use wire::message::Role;

use crate::SqliteStore;
use crate::error::SqliteError;
use crate::health::meta_value;
use crate::queue::ClaimedSqliteJob;
use crate::queue::EMBED_TASK;
use crate::queue::JobPayload;
use crate::runner::JobFailure;
use crate::runner::JobHandler;
use crate::runner::JobRunner;
use crate::runner::JobSweep;

mod reset;

const DEAD_SWEEP: chrono::Duration = chrono::Duration::minutes(10);
const BATCH: u32 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
enum EmbedTable {
    RosMessages,
    RosSummaries,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    table: EmbedTable,
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TargetRef {
    target_table: EmbedTable,
    target_id: String,
}

#[derive(Debug)]
pub struct EmbedRunner {
    store: SqliteStore,
    runner: JobRunner,
    model: Arc<Mutex<String>>,
    expected_dims: Arc<Mutex<Option<usize>>>,
}

impl EmbedRunner {
    pub fn attach(
        store: &SqliteStore,
        provider: Arc<dyn ModelProvider>,
        model: impl Into<String>,
        expected_dims: Option<usize>,
    ) -> Result<Self, SqliteError> {
        store.arm_embed();
        let model = Arc::new(Mutex::new(model.into()));
        let expected_dims = Arc::new(Mutex::new(expected_dims));
        let handler = Arc::new(EmbedHandler {
            store: store.clone(),
            provider,
            model: Arc::clone(&model),
            expected_dims: Arc::clone(&expected_dims),
        });
        let sweep = Arc::new(DeadEmbedSweep {
            store: store.clone(),
        });
        let mut runner = JobRunner::new(store.clone());
        runner.on_task(EMBED_TASK, handler);
        runner.on_sweep("embed-dead", DEAD_SWEEP, sweep);
        Ok(Self {
            store: store.clone(),
            runner,
            model,
            expected_dims,
        })
    }

    pub async fn configure_model(&self, model: &str) -> Result<(), SqliteError> {
        let stored = self.store.meta("embed_model").await?;
        match stored {
            Some(old) if old != model => self.store.record_log(format!(
                "[memory.sqlite] stored vectors were written by {old}, this process is configured for {model}; they are re-embedded only by a process that runs the job loop"
            )),
            Some(_) | None => {}
        }
        model.clone_into(&mut self.model.lock());
        Ok(())
    }

    pub fn set_expected_dims(&self, dims: Option<usize>) {
        *self.expected_dims.lock() = dims;
    }

    pub async fn run_jobs(&self, now: DateTime<Utc>) -> Result<u64, SqliteError> {
        self.reconcile_model(now).await?;
        self.runner.tick(now, BATCH).await
    }

    async fn reconcile_model(&self, now: DateTime<Utc>) -> Result<(), SqliteError> {
        let model = self.model.lock().clone();
        let stored = self.store.meta("embed_model").await?;
        let Some(old) = stored else {
            return Ok(());
        };
        if old == model {
            return Ok(());
        }
        self.store.record_log(format!(
            "[memory.sqlite] embedding model changed ({old} → {model}); re-embedding stored messages and summaries"
        ));
        let model = model.clone();
        self.store
            .with_conn(move |conn| reset::apply_model_change(conn, &model, now))
            .await
    }
}

struct EmbedHandler {
    store: SqliteStore,
    provider: Arc<dyn ModelProvider>,
    model: Arc<Mutex<String>>,
    expected_dims: Arc<Mutex<Option<usize>>>,
}

#[async_trait]
impl JobHandler for EmbedHandler {
    async fn run(&self, job: &ClaimedSqliteJob) -> Result<(), JobFailure> {
        match self.embed_one(job).await {
            Ok(()) => Ok(()),
            Err(error) => Err(JobFailure::new(error.to_string())),
        }
    }
}

impl EmbedHandler {
    async fn embed_one(&self, job: &ClaimedSqliteJob) -> Result<(), SqliteError> {
        let target = target_of(job)?;
        let text = load_text(&self.store, &target).await?;
        if let Some(reason) = reject(&text) {
            mark_unembeddable(&self.store, &target, reason).await?;
            return Ok(());
        }
        let vector = match request_vector(&self.provider, text).await {
            Ok(vector) => vector,
            Err(error) => {
                record_failure(&self.store, &target, error.to_string()).await?;
                return Err(error);
            }
        };
        let model = self.model.lock().clone();
        let expected = *self.expected_dims.lock();
        let target_for_write = target.clone();
        let job_id = job.id.clone();
        let written = self
            .store
            .with_conn(move |conn| {
                reset::write_vector(
                    conn,
                    &reset::StoreRequest {
                        target: &target_for_write,
                        vector: &vector,
                        model: &model,
                        expected,
                        job_id: &job_id,
                    },
                )
            })
            .await?;
        match written {
            reset::Written::Stored { width_change: None } => Ok(()),
            reset::Written::Stored {
                width_change: Some((from, to)),
            } => {
                self.store.record_log(format!(
                    "[memory.sqlite] embedding width changed ({from} → {to}); re-embedding stored messages and summaries"
                ));
                Ok(())
            }
            reset::Written::Failed(message) => Err(SqliteError::Embed { message }),
        }
    }
}

struct DeadEmbedSweep {
    store: SqliteStore,
}

#[async_trait]
impl JobSweep for DeadEmbedSweep {
    async fn run(&self, now: DateTime<Utc>) -> Result<(), JobFailure> {
        self.store
            .requeue_dead(Some(EMBED_TASK.to_owned()), now)
            .await
            .map(|_| ())
            .map_err(|error| JobFailure::new(error.to_string()))
    }
}

fn target_of(job: &ClaimedSqliteJob) -> Result<Target, SqliteError> {
    let JobPayload::Object(object) = &job.payload else {
        return Err(SqliteError::PayloadNotObject);
    };
    let value = serde_json::to_value(object).map_err(|source| SqliteError::Encode {
        column: "payload",
        source,
    })?;
    let parsed: TargetRef =
        serde_json::from_value(value).map_err(|source| SqliteError::StoredPayload { source })?;
    Ok(Target {
        table: parsed.target_table,
        id: parsed.target_id,
    })
}

fn reject(text: &str) -> Option<&'static str> {
    if text.trim().is_empty() {
        return Some("unembeddable: empty");
    }
    if base64_data_url(text) {
        return Some("unembeddable: base64-data-url");
    }
    None
}

fn base64_data_url(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("data:") else {
        return false;
    };
    let Some((meta, _)) = rest.split_once(',') else {
        return false;
    };
    meta.split(';')
        .any(|part| part.eq_ignore_ascii_case("base64"))
}

async fn load_text(store: &SqliteStore, target: &Target) -> Result<String, SqliteError> {
    let target = target.clone();
    store.with_conn(move |conn| read_text(conn, &target)).await
}

fn read_text(conn: &Connection, target: &Target) -> Result<String, SqliteError> {
    let text = match target.table {
        EmbedTable::RosMessages => message_text(conn, &target.id)?,
        EmbedTable::RosSummaries => summary_text(conn, &target.id)?,
    };
    text.ok_or(SqliteError::MissingEmbedTarget {
        table: table_name(target.table),
        id: target.id.clone(),
    })
}

fn message_text(conn: &Connection, id: &str) -> Result<Option<String>, SqliteError> {
    let row = conn
        .query_row(
            "SELECT role, content, tool_result FROM ros_messages WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(SqliteError::query("embed message"))?;
    let Some((role, content, tool_result)) = row else {
        return Ok(None);
    };
    let role = role
        .parse::<Role>()
        .map_err(|_| SqliteError::StoredRole { value: role })?;
    Ok(Some(embed_text(role, &content, tool_result.as_deref())))
}

fn summary_text(conn: &Connection, id: &str) -> Result<Option<String>, SqliteError> {
    conn.query_row(
        "SELECT content FROM ros_summaries WHERE id = ?1",
        [id],
        |row| row.get(0),
    )
    .optional()
    .map_err(SqliteError::query("embed summary"))
}

fn embed_text(role: Role, content: &str, tool_result: Option<&str>) -> String {
    match role {
        Role::Tool => match tool_result {
            Some(text) => text.to_owned(),
            None => String::new(),
        },
        Role::System | Role::User | Role::Assistant => content.to_owned(),
    }
}

async fn request_vector(
    provider: &Arc<dyn ModelProvider>,
    text: String,
) -> Result<Vec<f32>, SqliteError> {
    let embeddings = provider
        .embed(vec![text])
        .await
        .map_err(|error| provider_error(&error))?;
    match embeddings.as_slice() {
        [one] => Ok(one.vector.clone()),
        _ => Err(SqliteError::Embed {
            message: "embed endpoint returned a different number of vectors".to_owned(),
        }),
    }
}

fn provider_error(error: &ModelProviderError) -> SqliteError {
    let message = match &error {
        ModelProviderError::Request { message, .. }
        | ModelProviderError::Stream { message, .. } => message.as_str(),
    };
    SqliteError::Embed {
        message: match http_status(message) {
            Some(status) => format!("embed HTTP {status}"),
            None => message.to_owned(),
        },
    }
}

fn http_status(message: &str) -> Option<&str> {
    let status = message.strip_prefix("http ")?;
    if status.is_empty() || !status.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(status)
}

async fn record_failure(
    store: &SqliteStore,
    target: &Target,
    message: String,
) -> Result<(), SqliteError> {
    let target = target.clone();
    store
        .with_conn(move |conn| reset::mark_failure(conn, &target, &message))
        .await
}

async fn mark_unembeddable(
    store: &SqliteStore,
    target: &Target,
    reason: &'static str,
) -> Result<(), SqliteError> {
    let target = target.clone();
    store
        .with_conn(move |conn| {
            let sql = format!(
                "UPDATE {} SET embed_status = 'unembeddable', embed_error = ?2, embed_failures = 0
                  WHERE id = ?1",
                table_name(target.table)
            );
            let updated = conn
                .execute(&sql, params![target.id, reason])
                .map_err(SqliteError::query("unembeddable"))?;
            if updated == 0 {
                return Err(SqliteError::MissingEmbedTarget {
                    table: table_name(target.table),
                    id: target.id,
                });
            }
            Ok(())
        })
        .await
}

fn table_name(table: EmbedTable) -> String {
    table.to_string()
}

impl SqliteStore {
    async fn meta(&self, key: &str) -> Result<Option<String>, SqliteError> {
        let key = key.to_owned();
        self.with_conn(move |conn| meta_value(conn, &key)).await
    }
}
