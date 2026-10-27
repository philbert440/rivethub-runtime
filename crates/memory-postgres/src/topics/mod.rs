mod rows;
mod write;

use async_trait::async_trait;
use chrono::DateTime;
use chrono::Utc;
use kernel::topics::MergePlan;
use kernel::topics::PlacedSession;
use kernel::topics::Placement;
use kernel::topics::PlacementOutcome;
use kernel::topics::Redirect;
use kernel::topics::Slug;
use kernel::topics::Suggestion;
use kernel::topics::Topic;
use kernel::topics::TopicChanges;
use kernel::topics::TopicError;
use kernel::topics::TopicId;
use kernel::topics::TopicKey;
use kernel::topics::TopicListing;
use kernel::topics::TopicStore;
use kernel::topics::by_activity;
use kernel::topics::check_parent;
use wire::session_id::SessionId;

use crate::PostgresMemory;
use rows::AliasRow;
use rows::PlacedRow;
use rows::PlacementRow;
use rows::SuggestionRow;
use rows::TopicRow;
use rows::failed;
use rows::not_found;

#[async_trait]
impl TopicStore for PostgresMemory {
    async fn create(&self, topic: Topic) -> Result<Topic, TopicError> {
        let mut tx = self.pool.begin().await.map_err(failed("topic create"))?;
        if let Some(parent) = topic.parent {
            let parent = rows::by_id(&mut tx, parent)
                .await?
                .ok_or_else(|| not_found(parent))?;
            check_parent(topic.id, false, &parent)?;
        }
        write::claim_names(&mut tx, &topic).await?;
        write::insert(&mut tx, &topic).await?;
        write::aliases(&mut tx, &topic).await?;
        let stored = rows::by_id(&mut tx, topic.id)
            .await?
            .ok_or_else(|| not_found(topic.id))?;
        tx.commit().await.map_err(failed("topic create"))?;
        Ok(stored)
    }

    async fn update(
        &self,
        id: TopicId,
        changes: TopicChanges,
        now: DateTime<Utc>,
    ) -> Result<Topic, TopicError> {
        let mut tx = self.pool.begin().await.map_err(failed("topic update"))?;
        let current = rows::by_id(&mut tx, id)
            .await?
            .ok_or_else(|| not_found(id))?;
        let updated = current.apply(changes, now)?;
        if let Some(parent) = updated.parent {
            let parent = rows::by_id(&mut tx, parent)
                .await?
                .ok_or_else(|| not_found(parent))?;
            let subtopics = write::subtopics(&mut tx, id).await?;
            check_parent(id, !subtopics.is_empty(), &parent)?;
        }
        write::claim_names(&mut tx, &updated).await?;
        write::row(&mut tx, &updated).await?;
        write::aliases(&mut tx, &updated).await?;
        let stored = rows::by_id(&mut tx, id)
            .await?
            .ok_or_else(|| not_found(id))?;
        tx.commit().await.map_err(failed("topic update"))?;
        Ok(stored)
    }

    async fn merge(
        &self,
        source: TopicId,
        into: TopicId,
        now: DateTime<Utc>,
    ) -> Result<Topic, TopicError> {
        let mut tx = self.pool.begin().await.map_err(failed("topic merge"))?;
        let from = rows::by_id(&mut tx, source)
            .await?
            .ok_or_else(|| not_found(source))?;
        let target = rows::by_id(&mut tx, into)
            .await?
            .ok_or_else(|| not_found(into))?;
        let subtopics = write::subtopics(&mut tx, source).await?;
        let plan = MergePlan::new(&from, &target, &subtopics)?;
        let merged = plan.merged(target, now);
        write::fold_into(&mut tx, &plan, &merged).await?;
        let stored = rows::by_id(&mut tx, into)
            .await?
            .ok_or_else(|| not_found(into))?;
        tx.commit().await.map_err(failed("topic merge"))?;
        Ok(stored)
    }

    async fn find(&self, key: &TopicKey) -> Result<Option<Topic>, TopicError> {
        let mut conn = self.pool.acquire().await.map_err(failed("topic read"))?;
        match key {
            TopicKey::Id(id) => rows::by_id(&mut conn, *id).await,
            TopicKey::Slug(slug) => rows::by_slug(&mut conn, slug).await,
        }
    }

    async fn list(&self) -> Result<Vec<TopicListing>, TopicError> {
        let topics = sqlx::query!(
            r#"SELECT t.id, t.slug, t.name, t.parent_id, t.directory_kind, t.directory_node,
                      t.directory_path, t.created_at, t.updated_at,
                      (SELECT count(*) FROM rh_session_placements p WHERE p.topic_id = t.id)
                        AS "session_count!",
                      (SELECT max(c.updated_at)
                         FROM rh_session_placements p
                         JOIN rh_conversations c ON c.session_key = p.session_id
                        WHERE p.topic_id = t.id) AS last_active
                 FROM rh_topics t"#
        )
        .fetch_all(&self.pool)
        .await
        .map_err(failed("topic list"))?;
        let aliases = sqlx::query_as!(
            AliasRow,
            "SELECT topic_id, alias FROM rh_topic_aliases ORDER BY alias_key"
        )
        .fetch_all(&self.pool)
        .await
        .map_err(failed("topic list"))?;
        let mut aliases = rows::grouped(aliases)?;
        let mut listings = topics
            .into_iter()
            .map(|row| {
                let own = aliases.remove(&row.id).into_iter().flatten().collect();
                let topic = rows::topic(
                    TopicRow {
                        id: row.id,
                        slug: row.slug,
                        name: row.name,
                        parent_id: row.parent_id,
                        directory_kind: row.directory_kind,
                        directory_node: row.directory_node,
                        directory_path: row.directory_path,
                        created_at: row.created_at,
                        updated_at: row.updated_at,
                    },
                    own,
                )?;
                Ok(TopicListing {
                    topic,
                    session_count: u64::try_from(row.session_count).map_err(|source| {
                        TopicError::Unavailable(Box::new(crate::Error::Integer(source)))
                    })?,
                    last_active: row.last_active,
                })
            })
            .collect::<Result<Vec<_>, TopicError>>()?;
        listings.sort_by(by_activity);
        Ok(listings)
    }

    async fn redirects(&self) -> Result<Vec<Redirect>, TopicError> {
        sqlx::query!("SELECT slug, topic_id FROM rh_topic_redirects ORDER BY slug")
            .fetch_all(&self.pool)
            .await
            .map_err(failed("topic redirects"))?
            .into_iter()
            .map(|row| {
                Ok(Redirect {
                    slug: Slug::fold(&row.slug)?,
                    topic: TopicId::from_uuid(row.topic_id),
                })
            })
            .collect()
    }

    async fn place(&self, placement: Placement) -> Result<PlacementOutcome, TopicError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(failed("session placement"))?;
        write::lock_session(&mut tx, &placement.session).await?;
        if !write::topic_exists(&mut tx, placement.topic).await? {
            return Err(not_found(placement.topic));
        }
        let existing = rows::placement_of(&mut tx, &placement.session).await?;
        let outcome = match placement.admit(existing) {
            PlacementOutcome::Placed(placed) => {
                write::placement(&mut tx, &placed).await?;
                rows::placement_of(&mut tx, &placed.session)
                    .await?
                    .map(PlacementOutcome::Placed)
                    .ok_or_else(|| not_found(placed.topic))?
            }
            kept @ PlacementOutcome::Kept(_) => kept,
        };
        tx.commit().await.map_err(failed("session placement"))?;
        Ok(outcome)
    }

    async fn unplace(&self, session: &SessionId) -> Result<Option<Placement>, TopicError> {
        sqlx::query_as!(
            PlacementRow,
            "DELETE FROM rh_session_placements WHERE session_id = $1
             RETURNING session_id, topic_id, placed_by, placed_at",
            session.as_str()
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(failed("session unplace"))?
        .map(rows::placement)
        .transpose()
    }

    async fn placement(&self, session: &SessionId) -> Result<Option<Placement>, TopicError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(failed("session placement read"))?;
        rows::placement_of(&mut conn, session).await
    }

    async fn sessions(&self, topic: TopicId) -> Result<Vec<PlacedSession>, TopicError> {
        sqlx::query_as!(
            PlacedRow,
            "SELECT p.session_id, p.topic_id, p.placed_by, p.placed_at, c.last_active
               FROM rh_session_placements p
               LEFT JOIN LATERAL (
                   SELECT max(updated_at) AS last_active
                     FROM rh_conversations
                    WHERE session_key = p.session_id
               ) c ON true
              WHERE p.topic_id = $1
              ORDER BY c.last_active DESC NULLS LAST, p.session_id",
            topic.uuid()
        )
        .fetch_all(&self.pool)
        .await
        .map_err(failed("topic sessions"))?
        .into_iter()
        .map(rows::placed)
        .collect()
    }

    async fn suggest(&self, suggestion: Suggestion) -> Result<(), TopicError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(failed("session suggestion"))?;
        write::lock_session(&mut tx, &suggestion.session).await?;
        if rows::placement_of(&mut tx, &suggestion.session)
            .await?
            .is_none()
        {
            write::suggestion(&mut tx, &suggestion).await?;
        }
        tx.commit().await.map_err(failed("session suggestion"))?;
        Ok(())
    }

    async fn suggestion(&self, session: &SessionId) -> Result<Option<Suggestion>, TopicError> {
        sqlx::query_as!(
            SuggestionRow,
            "SELECT session_id, name, topic_id FROM rh_session_suggestions WHERE session_id = $1",
            session.as_str()
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(failed("session suggestion read"))?
        .map(rows::suggestion)
        .transpose()
    }

    async fn page_slug(&self, topic: TopicId) -> Result<Option<String>, TopicError> {
        sqlx::query_scalar!(
            "SELECT slug FROM rh_wiki_topics WHERE topic_id = $1",
            topic.uuid()
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(failed("topic page"))
    }
}
