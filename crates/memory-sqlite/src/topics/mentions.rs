use std::str::FromStr;

use kernel::filing::Mention;
use kernel::filing::conversation_keys;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use kernel::topics::TopicName;
use rusqlite::Connection;
use rusqlite::params;
use wire::session_id::SessionId;

use super::rows::failed;
use super::rows::stored;
use super::rows::topic_id;

const PLACED_SQL: &str = "SELECT session_id FROM rh_session_placements
     WHERE topic_id IN (SELECT value FROM json_each(?1))";

const MENTIONS_SQL: &str = "WITH filed(id) AS (
         SELECT c.id FROM ros_conversations c
          WHERE c.session_key IN (SELECT value FROM json_each(?1))
         UNION
         SELECT p.conversation_id FROM rh_conversation_placements p
          WHERE p.topic_id IN (SELECT value FROM json_each(?2))
     )
     SELECT t.id, t.name
       FROM ros_wiki_citations w
       JOIN ros_summaries s ON s.id = w.summary_id
       JOIN filed f ON f.id = s.conversation_id
       JOIN rh_topic_pages b ON b.page_slug = w.topic_slug
       JOIN rh_topics t ON t.id = b.topic_id AND t.kind = 'entity'
      GROUP BY t.id, t.name
      ORDER BY count(DISTINCT s.conversation_id) DESC, t.name
      LIMIT ?3";

pub(super) fn read(
    conn: &Connection,
    topics: &[TopicId],
    limit: u32,
) -> Result<Vec<Mention>, TopicError> {
    let ids = encoded(&topics.iter().map(ToString::to_string).collect::<Vec<_>>())?;
    let mut placed = conn.prepare(PLACED_SQL).map_err(failed("topic mentions"))?;
    let sessions = placed
        .query_map(params![ids], |row| row.get::<_, String>(0))
        .map_err(failed("topic mentions"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(failed("topic mentions"))?;
    let mut keys = Vec::new();
    for session in sessions {
        let session =
            SessionId::from_str(&session).map_err(|error| stored("session_id")(Box::new(error)))?;
        keys.extend(conversation_keys(&session));
    }
    let mut statement = conn
        .prepare(MENTIONS_SQL)
        .map_err(failed("topic mentions"))?;
    let found = statement
        .query_map(params![encoded(&keys)?, ids, limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(failed("topic mentions"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(failed("topic mentions"))?;
    found
        .into_iter()
        .map(|(entity, name)| {
            Ok(Mention {
                entity: topic_id(&entity)?,
                name: TopicName::new(&name)?,
            })
        })
        .collect()
}

fn encoded(values: &[String]) -> Result<String, TopicError> {
    serde_json::to_string(values).map_err(|error| stored("topic mentions")(Box::new(error)))
}
