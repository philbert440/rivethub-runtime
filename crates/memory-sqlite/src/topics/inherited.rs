use kernel::filing::FiledSession;
use kernel::topics::Lineage;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use rusqlite::Connection;
use rusqlite::params;
use uuid::Uuid;
use wire::session_id::SessionId;

use super::rows;
use super::rows::PlacementRow;
use super::rows::failed;
use super::rows::stored;
use super::rows::timestamp;

const INHERITED: &str = "
WITH RECURSIVE inherited (session_id, parent, task, node, topic_id, placed_by, placed_at, depth) AS (
    SELECT t.child_session_id, t.parent_session_id, t.id, t.node, p.topic_id, p.placed_by,
           p.placed_at, 1
      FROM rh_tasks t
      JOIN rh_session_placements p ON p.session_id = t.parent_session_id
     WHERE t.child_session_id IS NOT NULL AND (?1 IS NULL OR p.topic_id = ?1)
    UNION ALL
    SELECT t.child_session_id, t.parent_session_id, t.id, t.node, i.topic_id, i.placed_by,
           i.placed_at, i.depth + 1
      FROM rh_tasks t
      JOIN inherited i ON t.parent_session_id = i.session_id
     WHERE t.child_session_id IS NOT NULL AND i.depth < 8
       AND NOT EXISTS (SELECT 1 FROM rh_session_placements own WHERE own.session_id = i.session_id)
)
SELECT i.session_id, i.parent, i.task, i.node, i.topic_id, i.placed_by, i.placed_at,
       (SELECT c.title FROM ros_conversations c
         WHERE c.session_key = i.session_id AND c.title IS NOT NULL
         ORDER BY c.updated_at DESC LIMIT 1),
       (SELECT max(c.updated_at) FROM ros_conversations c WHERE c.session_key = i.session_id)
  FROM inherited i
 WHERE NOT EXISTS (SELECT 1 FROM rh_session_placements own WHERE own.session_id = i.session_id)
";

struct Row {
    placement: PlacementRow,
    parent: String,
    task: String,
    node: String,
    title: Option<String>,
    last_active: Option<String>,
}

pub(super) fn sessions(
    conn: &Connection,
    topic: Option<TopicId>,
) -> Result<Vec<FiledSession>, TopicError> {
    let mut statement = conn
        .prepare(INHERITED)
        .map_err(failed("inherited sessions"))?;
    let found = statement
        .query_map(params![topic.map(|topic| topic.to_string())], |row| {
            Ok(Row {
                placement: PlacementRow {
                    session_id: row.get(0)?,
                    topic_id: row.get(4)?,
                    placed_by: row.get(5)?,
                    placed_at: row.get(6)?,
                },
                parent: row.get(1)?,
                task: row.get(2)?,
                node: row.get(3)?,
                title: row.get(7)?,
                last_active: row.get(8)?,
            })
        })
        .map_err(failed("inherited sessions"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(failed("inherited sessions"))?;
    found.into_iter().map(session).collect()
}

fn session(row: Row) -> Result<FiledSession, TopicError> {
    Ok(FiledSession {
        placement: rows::placement(row.placement)?,
        title: row.title,
        last_active: row
            .last_active
            .as_deref()
            .map(|at| timestamp(at, "updated_at"))
            .transpose()?,
        node: Some(row.node),
        lineage: Some(Lineage {
            parent: SessionId::try_from(row.parent)
                .map_err(|error| stored("parent_session_id")(Box::new(error)))?,
            task: Uuid::parse_str(&row.task).map_err(|error| stored("task id")(Box::new(error)))?,
        }),
    })
}
