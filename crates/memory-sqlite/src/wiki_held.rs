use chrono::DateTime;
use chrono::Utc;
use kernel::filing::Target;
use std::str::FromStr;

use kernel::topics::Slug;
use kernel::topics::TopicId;
use kernel::wiki::builder::DeferredWiki;
use kernel::wiki::builder::ExtractWiki;
use kernel::wiki::builder::HeldEntity;
use kernel::wiki::builder::WikiStoreError;
use kernel::wiki::page::WikiPatch;
use kernel::wiki::page::WikiSlug;
use rusqlite::Connection;
use rusqlite::params;
use uuid::Uuid;
use wire::session_id::SessionId;

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
        "INSERT INTO rh_wiki_deferred
             (summary_id, waits_session, waits_conversation, applied_topic, patches, deferred_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (summary_id) DO UPDATE SET waits_session = excluded.waits_session,
            waits_conversation = excluded.waits_conversation, applied_topic = excluded.applied_topic,
            patches = excluded.patches, deferred_at = excluded.deferred_at",
        params![
            deferred.summary_id.to_string(),
            deferred.waits.session().map(SessionId::as_str),
            deferred.waits.conversation().map(|conversation| conversation.to_string()),
            deferred.applied.map(|topic| topic.to_string()),
            encoded(&deferred.patches)?,
            time::stamp(deferred.at)
        ],
    )
    .map_err(query)?;
    Ok(())
}

pub(crate) fn unsettled(
    conn: &Connection,
    limit: u32,
) -> Result<Vec<DeferredWiki>, WikiStoreError> {
    let rows = conn
        .prepare(
            "SELECT d.summary_id, d.waits_session, d.waits_conversation, d.applied_topic, d.patches,
                    d.deferred_at
               FROM rh_wiki_deferred d
               LEFT JOIN rh_session_placements p ON p.session_id = d.waits_session
               LEFT JOIN rh_conversation_placements c ON c.conversation_id = d.waits_conversation
              WHERE COALESCE(p.topic_id, c.topic_id) IS NOT d.applied_topic
              ORDER BY d.deferred_at, d.summary_id LIMIT ?1",
        )
        .and_then(|mut statement| {
            statement
                .query_map(params![limit], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(query)?;
    rows.into_iter()
        .map(
            |(summary_id, session, conversation, applied, patches, at)| {
                let conversation = conversation.as_deref().map(uuid).transpose()?;
                let applied = applied
                    .as_deref()
                    .map(|topic| TopicId::from_str(topic).map_err(invalid))
                    .transpose()?;
                Ok(DeferredWiki {
                    summary_id: uuid(&summary_id)?,
                    waits: Target::stored(session.as_deref(), conversation).ok_or_else(|| {
                        invalid("a deferral waits on neither a session nor a conversation")
                    })?,
                    applied,
                    patches: serde_json::from_str::<Vec<WikiPatch>>(&patches).map_err(invalid)?,
                    at: stamped(&at)?,
                })
            },
        )
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

pub(crate) fn uncite(
    conn: &mut Connection,
    summary_id: Uuid,
    slug: &WikiSlug,
) -> Result<(), WikiStoreError> {
    let tx = conn.transaction().map_err(query)?;
    for statement in [
        "DELETE FROM ros_wiki_citations WHERE topic_slug = ?1 AND summary_id = ?2
            AND NOT EXISTS (SELECT 1 FROM ros_wiki_extractions e, json_each(e.topics_touched) t
                             WHERE e.summary_id = ?2 AND t.value = ?1)",
        "DELETE FROM ros_wiki_provenance
          WHERE topic_slug = ?1 AND source_kind = 'summary' AND source_id = ?2
            AND NOT EXISTS (SELECT 1 FROM ros_wiki_extractions e, json_each(e.topics_touched) t
                             WHERE e.summary_id = ?2 AND t.value = ?1)",
    ] {
        tx.execute(statement, params![slug.as_ref(), summary_id.to_string()])
            .map_err(query)?;
    }
    tx.commit().map_err(query)
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
