use chrono::DateTime;
use chrono::Utc;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;

use crate::error::SqliteError;
use crate::health::meta_value;
use crate::queue::EMBED_TASK;
use crate::queue::enqueue_embed_due;
use crate::time;

use super::EmbedTable;
use super::Target;
use super::table_name;

pub(super) struct StoreRequest<'a> {
    pub(super) target: &'a Target,
    pub(super) vector: &'a [f32],
    pub(super) model: &'a str,
    pub(super) expected: Option<usize>,
    pub(super) job_id: &'a str,
}

pub(super) enum Written {
    Stored {
        width_change: Option<(usize, usize)>,
    },
    Failed(String),
}

pub(super) fn write_vector(
    conn: &mut Connection,
    request: &StoreRequest<'_>,
) -> Result<Written, SqliteError> {
    let stored = read_dims(conn)?;
    let got = request.vector.len();
    if let Some(have) = stored
        && have != got
    {
        return width_decision(conn, request, have);
    }
    if is_zero(request.vector) {
        return fail_vector(conn, request.target, "embedding is a zero vector");
    }
    let tx = conn
        .transaction()
        .map_err(SqliteError::query("embed store begin"))?;
    store_vector(&tx, request.target, request.vector, request.model, got)?;
    tx.commit()
        .map_err(SqliteError::query("embed store commit"))?;
    Ok(Written::Stored { width_change: None })
}

fn width_decision(
    conn: &mut Connection,
    request: &StoreRequest<'_>,
    have: usize,
) -> Result<Written, SqliteError> {
    let got = request.vector.len();
    if request.expected == Some(got) && !is_zero(request.vector) {
        return accept_width(conn, request, have, got);
    }
    if is_zero(request.vector) {
        return fail_vector(conn, request.target, "embedding is a zero vector");
    }
    let message = format!(
        "embedding is {got} wide but the store holds {have}-wide vectors; set embed_expected_dims to {got} to re-embed at the new width"
    );
    fail_vector(conn, request.target, &message)
}

fn accept_width(
    conn: &mut Connection,
    request: &StoreRequest<'_>,
    have: usize,
    got: usize,
) -> Result<Written, SqliteError> {
    let tx = conn
        .transaction()
        .map_err(SqliteError::query("embed width begin"))?;
    let cleared = other_embedded_ids(&tx, request.target)?;
    clear_other_vectors(&tx, request.target)?;
    let now = claim_clock(&tx, request.job_id)?;
    let run_at = just_after(now)?;
    rewrite_jobs(&tx, &cleared, now, run_at)?;
    store_vector(&tx, request.target, request.vector, request.model, got)?;
    tx.commit()
        .map_err(SqliteError::query("embed width commit"))?;
    Ok(Written::Stored {
        width_change: Some((have, got)),
    })
}

fn fail_vector(conn: &Connection, target: &Target, message: &str) -> Result<Written, SqliteError> {
    mark_failure(conn, target, message)?;
    Ok(Written::Failed(message.to_owned()))
}

pub(super) fn apply_model_change(
    conn: &mut Connection,
    model: &str,
    now: DateTime<Utc>,
) -> Result<(), SqliteError> {
    let tx = conn
        .transaction()
        .map_err(SqliteError::query("embed model begin"))?;
    let mut jobs = tagged(&tx, EmbedTable::RosMessages)?;
    jobs.extend(tagged(&tx, EmbedTable::RosSummaries)?);
    clear_embedded(&tx, EmbedTable::RosMessages)?;
    clear_embedded(&tx, EmbedTable::RosSummaries)?;
    upsert_meta(&tx, "embed_model", model)?;
    rewrite_jobs(&tx, &jobs, now, now)?;
    tx.commit()
        .map_err(SqliteError::query("embed model commit"))?;
    Ok(())
}

fn tagged(conn: &Connection, table: EmbedTable) -> Result<Vec<(EmbedTable, String)>, SqliteError> {
    Ok(embedded_ids(conn, table)?
        .into_iter()
        .map(|id| (table, id))
        .collect())
}

fn rewrite_jobs(
    conn: &Connection,
    jobs: &[(EmbedTable, String)],
    now: DateTime<Utc>,
    run_at: DateTime<Utc>,
) -> Result<(), SqliteError> {
    for (table, id) in jobs {
        drop_embed_job(conn, *table, id)?;
        enqueue_one(conn, *table, id, now, run_at)?;
    }
    Ok(())
}

fn drop_embed_job(conn: &Connection, table: EmbedTable, id: &str) -> Result<(), SqliteError> {
    let key = format!("embed-{}-{id}", table_name(table));
    conn.execute(
        "DELETE FROM ros_jobs WHERE task = ?1 AND job_key = ?2",
        params![EMBED_TASK, key],
    )
    .map_err(SqliteError::query("embed job drop"))?;
    Ok(())
}

fn enqueue_one(
    conn: &Connection,
    table: EmbedTable,
    id: &str,
    now: DateTime<Utc>,
    run_at: DateTime<Utc>,
) -> Result<(), SqliteError> {
    if enqueue_embed_due(conn, &table_name(table), id, now, run_at)? {
        Ok(())
    } else {
        Err(SqliteError::EmbedAlreadyQueued { id: id.to_owned() })
    }
}

fn embedded_ids(conn: &Connection, table: EmbedTable) -> Result<Vec<String>, SqliteError> {
    let sql = format!(
        "SELECT id FROM {} WHERE embedding IS NOT NULL ORDER BY rowid",
        table_name(table)
    );
    collect_ids(conn, &sql, &[])
}

fn other_embedded_ids(
    conn: &Connection,
    target: &Target,
) -> Result<Vec<(EmbedTable, String)>, SqliteError> {
    let mut jobs = Vec::new();
    for table in [EmbedTable::RosMessages, EmbedTable::RosSummaries] {
        let sql = format!(
            "SELECT id FROM {} WHERE embedding IS NOT NULL AND id <> ?1 ORDER BY rowid",
            table_name(table)
        );
        for id in collect_ids(conn, &sql, &[&target.id])? {
            jobs.push((table, id));
        }
    }
    Ok(jobs)
}

fn collect_ids(
    conn: &Connection,
    sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<Vec<String>, SqliteError> {
    let mut statement = conn
        .prepare(sql)
        .map_err(SqliteError::query("embedded ids"))?;
    let mut rows = statement
        .query(params)
        .map_err(SqliteError::query("embedded ids"))?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().map_err(SqliteError::query("embedded ids"))? {
        ids.push(row.get(0).map_err(SqliteError::query("embedded ids"))?);
    }
    Ok(ids)
}

fn clear_embedded(conn: &Connection, table: EmbedTable) -> Result<(), SqliteError> {
    let sql = format!(
        "UPDATE {} SET embedding = NULL, embed_status = NULL, embed_error = NULL, embed_failures = 0
          WHERE embedding IS NOT NULL",
        table_name(table)
    );
    conn.execute(&sql, [])
        .map_err(SqliteError::query("clear embeddings"))?;
    Ok(())
}

fn clear_other_vectors(conn: &Connection, target: &Target) -> Result<(), SqliteError> {
    for table in [EmbedTable::RosMessages, EmbedTable::RosSummaries] {
        let sql = format!(
            "UPDATE {} SET embedding = NULL, embed_status = NULL, embed_error = NULL, embed_failures = 0
              WHERE embedding IS NOT NULL AND id <> ?1",
            table_name(table)
        );
        conn.execute(&sql, [&target.id])
            .map_err(SqliteError::query("clear other embeddings"))?;
    }
    Ok(())
}

fn store_vector(
    conn: &Connection,
    target: &Target,
    vector: &[f32],
    model: &str,
    dims: usize,
) -> Result<(), SqliteError> {
    let bytes = pack_f32_le(&normalize(vector));
    let sql = format!(
        "UPDATE {} SET embedding = ?2, embed_status = 'done', embed_error = NULL, embed_failures = 0
          WHERE id = ?1",
        table_name(target.table)
    );
    let updated = conn
        .execute(&sql, params![target.id, bytes])
        .map_err(SqliteError::query("store embedding"))?;
    if updated == 0 {
        return Err(SqliteError::MissingEmbedTarget {
            table: table_name(target.table),
            id: target.id.clone(),
        });
    }
    upsert_meta(conn, "embed_model", model)?;
    upsert_meta(conn, "embed_dims", &dims.to_string())
}

pub(super) fn mark_failure(
    conn: &Connection,
    target: &Target,
    message: &str,
) -> Result<(), SqliteError> {
    let sql = format!(
        "UPDATE {} SET embed_error = ?2, embed_failures = embed_failures + 1 WHERE id = ?1",
        table_name(target.table)
    );
    let updated = conn
        .execute(&sql, params![target.id, message])
        .map_err(SqliteError::query("embed failure"))?;
    if updated == 0 {
        return Err(SqliteError::MissingEmbedTarget {
            table: table_name(target.table),
            id: target.id.clone(),
        });
    }
    Ok(())
}

fn claim_clock(conn: &Connection, job_id: &str) -> Result<DateTime<Utc>, SqliteError> {
    let locked = conn
        .query_row(
            "SELECT locked_at FROM ros_jobs WHERE id = ?1",
            [job_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(SqliteError::query("embed claim clock"))?;
    match locked {
        Some(Some(text)) => time::stored(&text, "locked_at"),
        Some(None) | None => Err(SqliteError::MissingJob {
            id: job_id.to_owned(),
        }),
    }
}

fn just_after(now: DateTime<Utc>) -> Result<DateTime<Utc>, SqliteError> {
    now.checked_add_signed(chrono::Duration::milliseconds(1))
        .ok_or(SqliteError::Clock)
}

fn read_dims(conn: &Connection) -> Result<Option<usize>, SqliteError> {
    let Some(value) = meta_value(conn, "embed_dims")? else {
        return Ok(None);
    };
    let dims = value
        .parse::<usize>()
        .map_err(|source| SqliteError::StoredDims {
            value: value.clone(),
            source,
        })?;
    Ok(Some(dims))
}

fn upsert_meta(conn: &Connection, key: &str, value: &str) -> Result<(), SqliteError> {
    conn.execute(
        "INSERT INTO ros_meta (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(SqliteError::query("meta upsert"))?;
    Ok(())
}

fn is_zero(vector: &[f32]) -> bool {
    vector.iter().all(|value| *value == 0.0)
}

fn normalize(vector: &[f32]) -> Vec<f32> {
    let mut sum = 0.0_f32;
    for value in vector {
        sum += value * value;
    }
    let norm = sum.sqrt();
    vector.iter().map(|value| value / norm).collect()
}

fn pack_f32_le(vector: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for value in vector {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}
