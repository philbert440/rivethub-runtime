use kernel::filing::Mention;
use kernel::filing::conversation_keys;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use kernel::topics::TopicName;
use sqlx::PgPool;
use uuid::Uuid;
use wire::session_id::SessionId;

use super::rows::failed;
use super::rows::stored;

const PLACED_SQL: &str =
    "SELECT session_id FROM rh_session_placements WHERE topic_id = ANY($1::uuid[])";

const MENTIONS_SQL: &str = "WITH filed AS (
         SELECT c.id FROM rh_conversations c WHERE c.session_key = ANY($1::text[])
         UNION
         SELECT p.conversation_id FROM rh_conversation_placements p
          WHERE p.topic_id = ANY($2::uuid[])
     )
     SELECT t.id, t.name
       FROM rh_wiki_citations w
       JOIN rh_summaries s ON s.id = w.summary_id
       JOIN filed f ON f.id = s.conversation_id
       JOIN rh_wiki_topics page ON page.slug = w.topic_slug
       JOIN rh_topics t ON t.id = page.topic_id AND t.kind = 'entity'
      GROUP BY t.id, t.name
      ORDER BY count(DISTINCT s.conversation_id) DESC, t.name
      LIMIT $3";

pub(super) async fn read(
    pool: &PgPool,
    topics: &[TopicId],
    limit: u32,
) -> Result<Vec<Mention>, TopicError> {
    let ids: Vec<Uuid> = topics.iter().copied().map(TopicId::uuid).collect();
    let placed: Vec<String> = sqlx::query_scalar(PLACED_SQL)
        .bind(&ids)
        .fetch_all(pool)
        .await
        .map_err(failed("topic mentions"))?;
    let mut keys = Vec::new();
    for session in placed {
        let session = session
            .parse::<SessionId>()
            .map_err(|error| stored("session_id")(Box::new(error)))?;
        keys.extend(conversation_keys(&session));
    }
    sqlx::query_as::<_, (Uuid, String)>(MENTIONS_SQL)
        .bind(&keys)
        .bind(&ids)
        .bind(i64::from(limit))
        .fetch_all(pool)
        .await
        .map_err(failed("topic mentions"))?
        .into_iter()
        .map(|(entity, name)| {
            Ok(Mention {
                entity: TopicId::from_uuid(entity),
                name: TopicName::new(&name)?,
            })
        })
        .collect()
}
