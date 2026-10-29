use chrono::DateTime;
use chrono::Utc;
use kernel::topics::Slug;
use kernel::wiki::builder::DeferredWiki;
use kernel::wiki::builder::ExtractWiki;
use kernel::wiki::builder::HeldEntity;
use kernel::wiki::builder::WikiPatch;
use kernel::wiki::builder::WikiStoreError;
use rusqlite::Connection;
use rusqlite::params;
use uuid::Uuid;
use wire::gateway::wiki::WikiSlug;

use crate::time;
use crate::wiki_builder::load_summary;
use crate::wiki_builder::query;

pub(crate) fn hold(
    conn: &Connection,
    held: &HeldEntity,
) -> Result<Vec<HeldEntity>, WikiStoreError> {
    conn.execute(
        "INSERT INTO rh_wiki_entity_candidates (key, source, summary_id, patch, held_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (key, source) DO UPDATE SET summary_id = excluded.summary_id,
            patch = excluded.patch, held_at = excluded.held_at",
        params![
            held.key.as_str(),
            held.source,
            held.summary_id.to_string(),
            encoded(&held.patch)?,
            time::stamp(held.at)
        ],
    )
    .map_err(query)?;
    let rows = conn
        .prepare(
            "SELECT source, summary_id, patch, held_at FROM rh_wiki_entity_candidates
              WHERE key = ?1 ORDER BY held_at, source",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![held.key.as_str()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(query)?;
    rows.into_iter()
        .map(|(source, summary_id, patch, at)| {
            Ok(HeldEntity {
                key: held.key.clone(),
                source,
                summary_id: uuid(&summary_id)?,
                patch: serde_json::from_str(&patch).map_err(invalid)?,
                at: stamped(&at)?,
            })
        })
        .collect()
}

pub(crate) fn release(conn: &Connection, key: &Slug) -> Result<(), WikiStoreError> {
    conn.execute(
        "DELETE FROM rh_wiki_entity_candidates WHERE key = ?1",
        params![key.as_str()],
    )
    .map_err(query)?;
    Ok(())
}

pub(crate) fn defer(conn: &Connection, deferred: &DeferredWiki) -> Result<(), WikiStoreError> {
    conn.execute(
        "INSERT INTO rh_wiki_deferred (summary_id, session_key, patches, deferred_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (summary_id) DO UPDATE SET session_key = excluded.session_key,
            patches = excluded.patches, deferred_at = excluded.deferred_at",
        params![
            deferred.summary_id.to_string(),
            deferred.session_key,
            encoded(&deferred.patches)?,
            time::stamp(deferred.at)
        ],
    )
    .map_err(query)?;
    Ok(())
}

pub(crate) fn filed(conn: &Connection, limit: u32) -> Result<Vec<DeferredWiki>, WikiStoreError> {
    let rows = conn
        .prepare(
            "SELECT d.summary_id, d.session_key, d.patches, d.deferred_at FROM rh_wiki_deferred d
              WHERE EXISTS (SELECT 1 FROM rh_session_placements p WHERE p.session_id = d.session_key)
              ORDER BY d.deferred_at, d.summary_id LIMIT ?1",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![limit], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(query)?;
    rows.into_iter()
        .map(|(summary_id, session_key, patches, at)| {
            Ok(DeferredWiki {
                summary_id: uuid(&summary_id)?,
                session_key,
                patches: serde_json::from_str::<Vec<WikiPatch>>(&patches).map_err(invalid)?,
                at: stamped(&at)?,
            })
        })
        .collect()
}

pub(crate) fn cite(
    conn: &Connection,
    summary_id: Uuid,
    slug: &WikiSlug,
    now: DateTime<Utc>,
) -> Result<(), WikiStoreError> {
    let job = ExtractWiki {
        summary_id,
        conversation_id: None,
    };
    let summary = load_summary(conn, &job)?;
    crate::wiki_topics::cite(conn, slug.as_ref(), &summary, now)
}

pub(crate) fn forget(conn: &Connection, summary_id: Uuid) -> Result<(), WikiStoreError> {
    conn.execute(
        "DELETE FROM rh_wiki_deferred WHERE summary_id = ?1",
        params![summary_id.to_string()],
    )
    .map_err(query)?;
    Ok(())
}

fn encoded<T: serde::Serialize>(value: &T) -> Result<String, WikiStoreError> {
    serde_json::to_string(value).map_err(invalid)
}

fn uuid(text: &str) -> Result<Uuid, WikiStoreError> {
    Uuid::parse_str(text).map_err(invalid)
}

fn stamped(text: &str) -> Result<DateTime<Utc>, WikiStoreError> {
    time::stored(text, "held_at").map_err(|error| WikiStoreError::Query(Box::new(error)))
}

fn invalid(error: impl std::fmt::Display) -> WikiStoreError {
    WikiStoreError::InvalidPatch(error.to_string())
}
