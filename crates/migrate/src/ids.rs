use rusqlite::OptionalExtension;
use rusqlite::Transaction;

const ROW_IDS: &[(&str, &str)] = &[
    ("ros_conversations", "id"),
    ("ros_conversations", "task_id"),
    ("ros_messages", "id"),
    ("ros_messages", "conversation_id"),
    ("ros_messages_fts", "id"),
    ("rh_messages_trigram", "id"),
    ("ros_summaries", "id"),
    ("ros_summaries", "conversation_id"),
    ("ros_summaries", "parent_id"),
    ("ros_summaries_fts", "id"),
    ("rh_summaries_trigram", "id"),
    ("ros_embed_queue", "id"),
    ("ros_embed_queue", "message_id"),
    ("ros_tags", "id"),
];

const LINK_IDS: &[(&str, &str)] = &[
    ("ros_summary_sources", "summary_id"),
    ("ros_summary_sources", "message_id"),
    ("ros_tags", "entity_id"),
    ("ros_wiki_provenance", "source_id"),
    ("ros_wiki_provenance", "conversation_id"),
    ("ros_wiki_extractions", "summary_id"),
    ("ros_wiki_citations", "summary_id"),
];

const JOB_KEY_ID: &str = "substr(job_key, -32)";

pub(crate) fn canonicalize(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
    let triggers =
        if undashed_rows(tx, "ros_messages", "id")? || undashed_rows(tx, "ros_summaries", "id")? {
            take_triggers(tx)?
        } else {
            Vec::new()
        };
    for (table, column) in ROW_IDS {
        tx.execute(
            &format!(
                "UPDATE {table} SET {column} = {} WHERE {}",
                dashed(column),
                undashed(column)
            ),
            [],
        )?;
    }
    for (table, column) in LINK_IDS {
        tx.execute(
            &format!(
                "UPDATE OR IGNORE {table} SET {column} = {} WHERE {}",
                dashed(column),
                undashed(column)
            ),
            [],
        )?;
        tx.execute(
            &format!("DELETE FROM {table} WHERE {}", undashed(column)),
            [],
        )?;
    }
    for key in ["targetId", "conversationId", "summaryId", "taskId"] {
        let value = format!("json_extract(payload, '$.{key}')");
        tx.execute(
            &format!(
                "UPDATE ros_jobs SET payload = json_set(payload, '$.{key}', {}) \
                  WHERE json_valid(payload) AND json_type(payload, '$.{key}') = 'text' AND {}",
                dashed(&value),
                undashed(&value)
            ),
            [],
        )?;
    }
    let keyed = format!(
        "length(job_key) > 33 AND substr(job_key, -33, 1) = '-' AND {}",
        undashed(JOB_KEY_ID)
    );
    tx.execute(
        &format!(
            "UPDATE OR IGNORE ros_jobs SET job_key = substr(job_key, 1, length(job_key) - 32) || {} \
              WHERE {keyed}",
            dashed(JOB_KEY_ID)
        ),
        [],
    )?;
    tx.execute(&format!("DELETE FROM ros_jobs WHERE {keyed}"), [])?;
    for sql in triggers {
        tx.execute_batch(&sql)?;
    }
    Ok(())
}

fn undashed(value: &str) -> String {
    format!("(length({value}) = 32 AND {value} NOT GLOB '*[^0-9A-Fa-f]*')")
}

fn dashed(value: &str) -> String {
    format!(
        "lower(substr({value}, 1, 8) || '-' || substr({value}, 9, 4) || '-' || \
         substr({value}, 13, 4) || '-' || substr({value}, 17, 4) || '-' || substr({value}, 21, 12))"
    )
}

fn undashed_rows(tx: &Transaction<'_>, table: &str, column: &str) -> rusqlite::Result<bool> {
    tx.query_row(
        &format!("SELECT 1 FROM {table} WHERE {} LIMIT 1", undashed(column)),
        [],
        |row| row.get::<_, i64>(0),
    )
    .optional()
    .map(|found| found.is_some())
}

fn take_triggers(tx: &Transaction<'_>) -> rusqlite::Result<Vec<String>> {
    let mut taken = Vec::new();
    for name in ["ros_messages_au", "ros_summaries_au"] {
        let sql: Option<String> = tx
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(sql) = sql {
            tx.execute_batch(&format!("DROP TRIGGER {name}"))?;
            taken.push(sql);
        }
    }
    Ok(taken)
}
