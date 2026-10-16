use std::collections::BTreeSet;

use chrono::DateTime;
use chrono::Utc;
use kernel::wiki::builder::WikiAction;
use kernel::wiki::builder::WikiPatch;
use kernel::wiki::builder::WikiStoreError;
use kernel::wiki::builder::WikiSummary;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::params;
use thiserror::Error;

use crate::time;

#[derive(Debug, Error)]
#[error("{0}")]
struct Note(String);

struct Existing {
    title: String,
    aliases: String,
    tags: String,
    entities: String,
    current_state: String,
    article: String,
    related: String,
}

pub(crate) fn apply(
    conn: &Connection,
    summary: &WikiSummary,
    patch: &WikiPatch,
    now: DateTime<Utc>,
) -> Result<Option<String>, WikiStoreError> {
    let slug = canonical(conn, patch)?;
    let aliases = alias_list(patch, &slug);
    let at = time::stamp(now);
    let existing = load(conn, &slug)?;
    let article = stored_article(patch.article.as_deref(), existing.as_ref());
    let related = stored_related(patch.related.as_ref(), existing.as_ref())?;
    let aliases_json = json(&aliases)?;
    let tags_json = json(&patch.tags)?;
    let entities_json = json(&patch.entities)?;
    let search = search_text(patch, &aliases, patch.article.as_deref(), existing.as_ref());
    let changed = write_topic(
        conn,
        &slug,
        patch,
        &TopicWrite {
            aliases: aliases_json,
            tags: tags_json,
            entities: entities_json,
            article,
            related,
            search,
            history: i64::from(patch.history_entry.is_some()),
            at: at.clone(),
        },
        existing.is_some(),
    )?;
    redirects(conn, &slug, &aliases, &at)?;
    provenance(
        conn,
        &slug,
        summary,
        patch
            .history_entry
            .as_ref()
            .map(|entry| entry.body.as_str()),
        &at,
    )?;
    Ok(changed.then_some(slug))
}

struct TopicWrite {
    aliases: String,
    tags: String,
    entities: String,
    article: String,
    related: String,
    search: String,
    history: i64,
    at: String,
}

fn write_topic(
    conn: &Connection,
    slug: &str,
    patch: &WikiPatch,
    write: &TopicWrite,
    exists: bool,
) -> Result<bool, WikiStoreError> {
    if exists && unchanged(conn, slug, patch, write)? {
        return Ok(false);
    }
    let rows = if exists {
        update_topic(conn, slug, patch, write)?
    } else {
        insert_topic(conn, slug, patch, write)?
    };
    if rows == 1 {
        Ok(true)
    } else {
        Err(WikiStoreError::Query(Box::new(Note(format!(
            "wiki topic {slug} was not written"
        )))))
    }
}

fn unchanged(
    conn: &Connection,
    slug: &str,
    patch: &WikiPatch,
    write: &TopicWrite,
) -> Result<bool, WikiStoreError> {
    let Some(row) = load(conn, slug)? else {
        return Ok(false);
    };
    let state_matches = row.current_state == patch.summary;
    Ok(row.title == patch.title
        && row.aliases == write.aliases
        && row.tags == write.tags
        && row.entities == write.entities
        && row.article == write.article
        && row.related == write.related
        && state_matches)
}

fn insert_topic(
    conn: &Connection,
    slug: &str,
    patch: &WikiPatch,
    write: &TopicWrite,
) -> Result<usize, WikiStoreError> {
    conn.execute(
        "INSERT INTO ros_wiki_topics (
            slug, title, aliases, tags, entities, related, current_state, article, search_text,
            history_count, last_verified_at, created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11, ?11)",
        params![
            slug,
            patch.title,
            write.aliases,
            write.tags,
            write.entities,
            write.related,
            patch.summary,
            write.article,
            write.search,
            write.history,
            write.at
        ],
    )
    .map_err(query)
}

fn update_topic(
    conn: &Connection,
    slug: &str,
    patch: &WikiPatch,
    write: &TopicWrite,
) -> Result<usize, WikiStoreError> {
    conn.execute(
        "UPDATE ros_wiki_topics SET title = ?2, aliases = ?3, tags = ?4, entities = ?5,
            related = ?6, current_state = ?7, article = ?8, search_text = ?9,
            history_count = history_count + ?10, last_verified_at = ?11, updated_at = ?11,
            embedding = NULL, embed_status = NULL, embed_failures = 0, embed_error = NULL
         WHERE slug = ?1",
        params![
            slug,
            patch.title,
            write.aliases,
            write.tags,
            write.entities,
            write.related,
            patch.summary,
            write.article,
            write.search,
            write.history,
            write.at
        ],
    )
    .map_err(query)
}

fn load(conn: &Connection, slug: &str) -> Result<Option<Existing>, WikiStoreError> {
    conn.query_row(
        "SELECT title, aliases, tags, entities, current_state, article, related
           FROM ros_wiki_topics WHERE slug = ?1",
        params![slug],
        |row| {
            Ok(Existing {
                title: row.get(0)?,
                aliases: row.get(1)?,
                tags: row.get(2)?,
                entities: row.get(3)?,
                current_state: row.get(4)?,
                article: row.get(5)?,
                related: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(query)
}

fn canonical(conn: &Connection, patch: &WikiPatch) -> Result<String, WikiStoreError> {
    let mut candidates = vec![patch.slug.as_ref().to_owned()];
    candidates.extend(patch.aliases.iter().map(|alias| alias.as_ref().to_owned()));
    let mut found = BTreeSet::new();
    for candidate in &candidates {
        for slug in matches(conn, candidate)? {
            found.insert(slug);
        }
    }
    let mut found = found.into_iter();
    match (found.next(), found.next()) {
        (None, _) if patch.action == WikiAction::Create => Ok(patch.slug.as_ref().to_owned()),
        (None, _) => Err(WikiStoreError::TopicMissing(patch.slug.as_ref().to_owned())),
        (Some(slug), None) => Ok(slug),
        (Some(_), Some(_)) => Err(WikiStoreError::SlugConflict(patch.slug.as_ref().to_owned())),
    }
}

fn matches(conn: &Connection, candidate: &str) -> Result<Vec<String>, WikiStoreError> {
    let mut statement = conn
        .prepare(
            "SELECT slug FROM ros_wiki_topics WHERE slug = ?1
             UNION
             SELECT t.slug FROM ros_wiki_topics t, json_each(t.aliases) AS item
              WHERE item.value = ?1
             UNION
             SELECT to_slug FROM ros_wiki_redirects WHERE from_slug = ?1",
        )
        .map_err(query)?;
    let rows = statement
        .query_map(params![candidate], |row| row.get(0))
        .map_err(query)?;
    let mut slugs = Vec::new();
    for row in rows {
        slugs.push(row.map_err(query)?);
    }
    Ok(slugs)
}

fn redirects(
    conn: &Connection,
    slug: &str,
    aliases: &[String],
    at: &str,
) -> Result<(), WikiStoreError> {
    for alias in aliases {
        let occupied: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM ros_wiki_topics WHERE slug = ?1)",
                params![alias],
                |row| row.get(0),
            )
            .map_err(query)?;
        if occupied != 0 {
            return Err(WikiStoreError::SlugConflict(alias.clone()));
        }
        let written = conn
            .execute(
                "INSERT INTO ros_wiki_redirects (from_slug, to_slug, created_at)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(from_slug) DO UPDATE SET to_slug = excluded.to_slug
                 WHERE ros_wiki_redirects.to_slug = excluded.to_slug",
                params![alias, slug, at],
            )
            .map_err(query)?;
        if written != 1 {
            return Err(WikiStoreError::SlugConflict(alias.clone()));
        }
    }
    Ok(())
}

fn provenance(
    conn: &Connection,
    slug: &str,
    summary: &WikiSummary,
    note: Option<&str>,
    at: &str,
) -> Result<(), WikiStoreError> {
    let conversation = summary.conversation_id.map(|id| id.to_string());
    conn.execute(
        "INSERT INTO ros_wiki_provenance
            (topic_slug, source_kind, source_id, conversation_id, created_at)
         VALUES (?1, 'summary', ?2, ?3, ?4) ON CONFLICT DO NOTHING",
        params![slug, summary.id.to_string(), conversation, at],
    )
    .map_err(query)?;
    conn.execute(
        "INSERT INTO ros_wiki_citations (topic_slug, summary_id, kind, note, cited_at)
         VALUES (?1, ?2, 'summary', ?3, ?4) ON CONFLICT DO NOTHING",
        params![slug, summary.id.to_string(), note, at],
    )
    .map_err(query)?;
    Ok(())
}

fn alias_list(patch: &WikiPatch, slug: &str) -> Vec<String> {
    let mut aliases: Vec<String> = patch
        .aliases
        .iter()
        .map(|alias| alias.as_ref().to_owned())
        .chain((slug != patch.slug.as_ref()).then(|| patch.slug.as_ref().to_owned()))
        .filter(|alias| alias != slug)
        .collect();
    aliases.sort();
    aliases.dedup();
    aliases
}

fn stored_article(article: Option<&str>, existing: Option<&Existing>) -> String {
    match article {
        Some(text) => text.to_owned(),
        None => match existing {
            Some(row) => row.article.clone(),
            None => String::new(),
        },
    }
}

fn stored_related(
    related: Option<&Vec<wire::gateway::wiki::WikiSlug>>,
    existing: Option<&Existing>,
) -> Result<String, WikiStoreError> {
    match related {
        Some(slugs) => {
            let values = slugs
                .iter()
                .map(|slug| slug.as_ref().to_owned())
                .collect::<Vec<_>>();
            json(&values)
        }
        None => match existing {
            Some(row) => Ok(row.related.clone()),
            None => Ok("[]".to_owned()),
        },
    }
}

fn search_text(
    patch: &WikiPatch,
    aliases: &[String],
    article: Option<&str>,
    existing: Option<&Existing>,
) -> String {
    let prefix = format!(
        "{}\n{}\n{}\n{}\n{}",
        patch.title,
        patch.summary,
        aliases.join(" "),
        patch.tags.join(" "),
        patch.entities.join(" ")
    );
    match article {
        Some(text) => format!("{prefix}\n{text}"),
        None => match existing {
            Some(row) => format!("{prefix}\n{}", row.article),
            None => prefix,
        },
    }
}

fn json(values: &[String]) -> Result<String, WikiStoreError> {
    serde_json::to_string(values).map_err(|source| WikiStoreError::Query(Box::new(source)))
}

fn query(source: rusqlite::Error) -> WikiStoreError {
    WikiStoreError::Query(Box::new(source))
}
