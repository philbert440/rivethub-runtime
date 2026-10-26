use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::DateTime;
use chrono::Utc;
use kernel::topics::Alias;
use kernel::topics::PlacedSession;
use kernel::topics::Placement;
use kernel::topics::PlacementSource;
use kernel::topics::Root;
use kernel::topics::RootKind;
use kernel::topics::Slug;
use kernel::topics::Topic;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use kernel::topics::TopicName;
use sqlx::PgConnection;
use uuid::Uuid;
use wire::session_id::SessionId;

use crate::error::Error;

pub(super) struct TopicRow {
    pub(super) id: Uuid,
    pub(super) slug: String,
    pub(super) name: String,
    pub(super) parent_id: Option<Uuid>,
    pub(super) directory_kind: Option<String>,
    pub(super) directory_node: Option<String>,
    pub(super) directory_path: Option<String>,
    pub(super) created_at: DateTime<Utc>,
    pub(super) updated_at: DateTime<Utc>,
}

pub(super) struct AliasRow {
    pub(super) topic_id: Uuid,
    pub(super) alias: String,
}

pub(super) struct PlacementRow {
    pub(super) session_id: String,
    pub(super) topic_id: Uuid,
    pub(super) placed_by: String,
    pub(super) placed_at: DateTime<Utc>,
}

pub(super) struct PlacedRow {
    pub(super) session_id: String,
    pub(super) topic_id: Uuid,
    pub(super) placed_by: String,
    pub(super) placed_at: DateTime<Utc>,
    pub(super) last_active: Option<DateTime<Utc>>,
}

pub(super) fn failed(operation: &'static str) -> impl FnOnce(sqlx::Error) -> TopicError {
    move |source| TopicError::Unavailable(Box::new(Error::operation(operation, source)))
}

fn stored(
    field: &'static str,
) -> impl FnOnce(Box<dyn std::error::Error + Send + Sync>) -> TopicError {
    move |source| TopicError::Unavailable(Box::new(Error::StoredValue { field, source }))
}

pub(super) fn topic(row: TopicRow, aliases: Vec<Alias>) -> Result<Topic, TopicError> {
    let directory = match row.directory_kind {
        None => None,
        Some(kind) => {
            let kind = RootKind::from_str(&kind)
                .map_err(|error| stored("directory_kind")(Box::new(error)))?;
            let path = row
                .directory_path
                .ok_or_else(|| stored("directory_path")("a directory without a path".into()))?;
            Some(
                Root::from_parts(kind, row.directory_node, path).ok_or_else(|| {
                    stored("directory_node")("a directory node that does not match its kind".into())
                })?,
            )
        }
    };
    Ok(Topic {
        id: TopicId::from_uuid(row.id),
        slug: Slug::fold(&row.slug)?,
        name: TopicName::new(&row.name)?,
        aliases,
        parent: row.parent_id.map(TopicId::from_uuid),
        directory,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

pub(super) fn placement(row: PlacementRow) -> Result<Placement, TopicError> {
    Ok(Placement {
        session: SessionId::try_from(row.session_id)
            .map_err(|error| stored("session_id")(Box::new(error)))?,
        topic: TopicId::from_uuid(row.topic_id),
        by: PlacementSource::from_str(&row.placed_by)
            .map_err(|error| stored("placed_by")(Box::new(error)))?,
        at: row.placed_at,
    })
}

pub(super) fn placed(row: PlacedRow) -> Result<PlacedSession, TopicError> {
    Ok(PlacedSession {
        last_active: row.last_active,
        placement: placement(PlacementRow {
            session_id: row.session_id,
            topic_id: row.topic_id,
            placed_by: row.placed_by,
            placed_at: row.placed_at,
        })?,
    })
}

pub(super) fn grouped(rows: Vec<AliasRow>) -> Result<BTreeMap<Uuid, Vec<Alias>>, TopicError> {
    let mut grouped: BTreeMap<Uuid, Vec<Alias>> = BTreeMap::new();
    for row in rows {
        grouped
            .entry(row.topic_id)
            .or_default()
            .push(Alias::new(&row.alias)?);
    }
    Ok(grouped)
}

pub(super) async fn aliases_of(
    conn: &mut PgConnection,
    id: Uuid,
) -> Result<Vec<Alias>, TopicError> {
    let rows = sqlx::query_as!(
        AliasRow,
        "SELECT topic_id, alias FROM rh_topic_aliases WHERE topic_id = $1 ORDER BY alias_key",
        id
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(failed("topic aliases"))?;
    rows.iter().map(|row| Alias::new(&row.alias)).collect()
}

pub(super) async fn by_id(
    conn: &mut PgConnection,
    id: TopicId,
) -> Result<Option<Topic>, TopicError> {
    let row = sqlx::query_as!(
        TopicRow,
        "SELECT id, slug, name, parent_id, directory_kind, directory_node, directory_path,
                created_at, updated_at
           FROM rh_topics WHERE id = $1 FOR UPDATE",
        id.uuid()
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(failed("topic read"))?;
    match row {
        Some(row) => {
            let aliases = aliases_of(conn, row.id).await?;
            topic(row, aliases).map(Some)
        }
        None => Ok(None),
    }
}

pub(super) async fn by_slug(
    conn: &mut PgConnection,
    slug: &Slug,
) -> Result<Option<Topic>, TopicError> {
    let row = sqlx::query_as!(
        TopicRow,
        "SELECT t.id, t.slug, t.name, t.parent_id, t.directory_kind, t.directory_node,
                t.directory_path, t.created_at, t.updated_at
           FROM rh_topics t
          WHERE t.slug = $1
             OR t.id = (SELECT r.topic_id FROM rh_topic_redirects r WHERE r.slug = $1)
          ORDER BY (t.slug = $1) DESC
          LIMIT 1",
        slug.as_str()
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(failed("topic slug lookup"))?;
    match row {
        Some(row) => {
            let aliases = aliases_of(conn, row.id).await?;
            topic(row, aliases).map(Some)
        }
        None => Ok(None),
    }
}

pub(super) async fn placement_of(
    conn: &mut PgConnection,
    session: &SessionId,
) -> Result<Option<Placement>, TopicError> {
    sqlx::query_as!(
        PlacementRow,
        "SELECT session_id, topic_id, placed_by, placed_at
           FROM rh_session_placements WHERE session_id = $1",
        session.as_str()
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(failed("session placement read"))?
    .map(placement)
    .transpose()
}

pub(super) fn not_found(id: TopicId) -> TopicError {
    TopicError::NotFound {
        key: id.to_string(),
    }
}
