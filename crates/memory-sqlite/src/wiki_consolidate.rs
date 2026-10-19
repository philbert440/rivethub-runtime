use std::collections::BTreeSet;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::wiki::builder::WikiStoreError;
use kernel::wiki::consolidate::WikiConsolidateStore;
use kernel::wiki::consolidate::WikiFold;
use rusqlite::Connection;
use rusqlite::OptionalExtension as _;
use rusqlite::TransactionBehavior;
use rusqlite::params;

use crate::SqliteStore;
use crate::wiki_builder::blocking;
use crate::wiki_builder::query;

struct Topic {
    slug: String,
    title: String,
    aliases: Vec<String>,
    tags: Vec<String>,
    entities: Vec<String>,
    related: Vec<String>,
    current_state: String,
    article: String,
    history_count: i64,
}

#[derive(Default)]
struct Leftovers {
    superseded: Vec<String>,
    merged: Vec<String>,
}

#[async_trait]
impl WikiConsolidateStore for SqliteStore {
    async fn wiki_slugs(&self) -> Result<Vec<String>, WikiStoreError> {
        blocking(self, |conn| slugs(conn)).await
    }

    async fn fold_wiki(
        &self,
        fold: &WikiFold,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, WikiStoreError> {
        let fold = fold.clone();
        blocking(self, move |conn| fold_all(conn, &fold, now)).await
    }
}

fn slugs(conn: &Connection) -> Result<Vec<String>, WikiStoreError> {
    let mut statement = conn
        .prepare("SELECT slug FROM ros_wiki_topics ORDER BY slug")
        .map_err(query)?;
    let rows = statement.query_map([], |row| row.get(0)).map_err(query)?;
    rows.collect::<Result<Vec<String>, _>>().map_err(query)
}

fn fold_all(
    conn: &mut Connection,
    fold: &WikiFold,
    now: DateTime<Utc>,
) -> Result<Vec<String>, WikiStoreError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(query)?;
    let mut parent = load(&tx, &fold.parent)?
        .ok_or_else(|| WikiStoreError::TopicMissing(fold.parent.clone()))?;
    let mut folded = Vec::new();
    for slug in &fold.children {
        if let Some(child) = load(&tx, slug)? {
            absorb(&mut parent, child);
            folded.push(slug.clone());
        }
    }
    if folded.is_empty() {
        tx.commit().map_err(query)?;
        return Ok(folded);
    }
    let at = crate::time::stamp(now);
    store_parent(&tx, &parent, &at)?;
    for child in &folded {
        retire(&tx, child, &parent.slug, &at)?;
    }
    tx.commit().map_err(query)?;
    Ok(folded)
}

fn absorb(parent: &mut Topic, child: Topic) {
    let mut aliases: BTreeSet<String> = std::mem::take(&mut parent.aliases).into_iter().collect();
    aliases.insert(child.slug.clone());
    aliases.extend(child.aliases);
    aliases.remove(&parent.slug);
    parent.aliases = aliases.into_iter().collect();
    merge_unique(&mut parent.tags, child.tags);
    merge_unique(&mut parent.entities, child.entities);
    let related: Vec<String> = child
        .related
        .into_iter()
        .filter(|slug| *slug != parent.slug)
        .collect();
    merge_unique(&mut parent.related, related);
    parent.history_count += child.history_count;
    let mut leftovers = Leftovers::default();
    take_denser(
        &mut parent.current_state,
        child.current_state,
        "summary",
        &mut leftovers,
    );
    take_denser(
        &mut parent.article,
        child.article,
        "article",
        &mut leftovers,
    );
    if !leftovers.superseded.is_empty() {
        append_entry(
            &mut parent.article,
            &format!(
                "#### Superseded parent text\n\n{}",
                leftovers.superseded.join("\n\n")
            ),
        );
    }
    let mut merged = format!("#### Merged from {}\n\n**{}**", child.slug, child.title);
    for text in leftovers.merged {
        merged.push_str("\n\n");
        merged.push_str(&text);
    }
    append_entry(&mut parent.article, &merged);
}

fn take_denser(slot: &mut String, incoming: String, label: &str, leftovers: &mut Leftovers) {
    if incoming.chars().count() > slot.chars().count() {
        let displaced = std::mem::replace(slot, incoming);
        if !displaced.trim().is_empty() {
            leftovers.superseded.push(format!(
                "**Displaced parent {label}:**\n\n{}",
                displaced.trim()
            ));
        }
    } else if !incoming.trim().is_empty() && incoming.trim() != slot.trim() {
        leftovers.merged.push(incoming.trim().to_owned());
    }
}

fn append_entry(article: &mut String, entry: &str) {
    if article.contains(entry) {
        return;
    }
    let kept = article.trim_end();
    *article = if kept.is_empty() {
        entry.to_owned()
    } else {
        format!("{kept}\n\n{entry}")
    };
}

fn merge_unique(existing: &mut Vec<String>, additions: Vec<String>) {
    for item in additions {
        if !item.is_empty() && !existing.contains(&item) {
            existing.push(item);
        }
    }
}

fn load(conn: &Connection, slug: &str) -> Result<Option<Topic>, WikiStoreError> {
    let row = conn
        .query_row(
            "SELECT slug, title, aliases, tags, entities, related, current_state, article,
                    history_count
               FROM ros_wiki_topics WHERE slug = ?1",
            params![slug],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    [
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ],
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, i64>(8)?,
                ))
            },
        )
        .optional()
        .map_err(query)?;
    let Some((slug, title, [aliases, tags, entities, related], current_state, article, history)) =
        row
    else {
        return Ok(None);
    };
    Ok(Some(Topic {
        slug,
        title,
        aliases: list(&aliases)?,
        tags: list(&tags)?,
        entities: list(&entities)?,
        related: list(&related)?,
        current_state,
        article,
        history_count: history,
    }))
}

fn store_parent(conn: &Connection, topic: &Topic, at: &str) -> Result<(), WikiStoreError> {
    let search = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        topic.title,
        topic.current_state,
        topic.aliases.join(" "),
        topic.tags.join(" "),
        topic.entities.join(" "),
        topic.article
    );
    conn.execute(
        "UPDATE ros_wiki_topics SET aliases = ?2, tags = ?3, entities = ?4, related = ?5,
            current_state = ?6, article = ?7, search_text = ?8, history_count = ?9,
            updated_at = ?10, embedding = NULL, embed_status = NULL, embed_failures = 0,
            embed_error = NULL
         WHERE slug = ?1",
        params![
            topic.slug,
            json(&topic.aliases)?,
            json(&topic.tags)?,
            json(&topic.entities)?,
            json(&topic.related)?,
            topic.current_state,
            topic.article,
            search,
            topic.history_count,
            at
        ],
    )
    .map_err(query)?;
    Ok(())
}

fn retire(conn: &Connection, child: &str, parent: &str, at: &str) -> Result<(), WikiStoreError> {
    conn.execute(
        "INSERT INTO ros_wiki_citations (topic_slug, summary_id, kind, note, cited_at)
         SELECT ?2, summary_id, kind, note, cited_at FROM ros_wiki_citations
          WHERE topic_slug = ?1
         ON CONFLICT DO NOTHING",
        params![child, parent],
    )
    .map_err(query)?;
    conn.execute(
        "INSERT INTO ros_wiki_provenance
            (topic_slug, source_kind, source_id, conversation_id, git_sha, created_at)
         SELECT ?2, source_kind, source_id, conversation_id, git_sha, created_at
           FROM ros_wiki_provenance WHERE topic_slug = ?1
         ON CONFLICT DO NOTHING",
        params![child, parent],
    )
    .map_err(query)?;
    conn.execute(
        "UPDATE ros_wiki_redirects SET to_slug = ?2 WHERE to_slug = ?1",
        params![child, parent],
    )
    .map_err(query)?;
    conn.execute(
        "INSERT INTO ros_wiki_redirects (from_slug, to_slug, created_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(from_slug) DO UPDATE SET to_slug = excluded.to_slug",
        params![child, parent, at],
    )
    .map_err(query)?;
    conn.execute(
        "DELETE FROM ros_wiki_redirects WHERE from_slug = ?1",
        params![parent],
    )
    .map_err(query)?;
    conn.execute(
        "DELETE FROM ros_wiki_topics WHERE slug = ?1",
        params![child],
    )
    .map_err(query)?;
    Ok(())
}

fn list(text: &str) -> Result<Vec<String>, WikiStoreError> {
    serde_json::from_str(text).map_err(|source| WikiStoreError::Query(Box::new(source)))
}

fn json(values: &[String]) -> Result<String, WikiStoreError> {
    serde_json::to_string(values).map_err(|source| WikiStoreError::Query(Box::new(source)))
}
